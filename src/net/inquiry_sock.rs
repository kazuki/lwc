use super::{DatagramSocket, InquirySocket, Serializable, SocketEventType, RetransmissionTimerAlgorithm};
use ::thread::{Cron};
use ::io::{SerDe};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::io::{Error, Result};
use std::marker::{Sync, Send, PhantomData};
use std::sync::{Arc, RwLock};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::{JoinHandle, spawn};
use rustc_serialize::{Encoder, Encodable, Decoder};
use rand::{IsaacRng, Rng, SeedableRng, thread_rng};
use time;

/// データグラム上で`InquirySocket`を実現する
pub struct SimpleInquirySocket<DS: DatagramSocket, SERDE: SerDe, TREQ: Serializable, TRES: Serializable, RTO: RetransmissionTimerAlgorithm<IpAddr>> {
    sock: Arc<DS>,
    serde: Arc<SERDE>,
    exit_flag: Arc<AtomicBool>,
    recv_thread: Option<JoinHandle<()>>,
    reqid_rng: IsaacRng,
    waiting_map: Arc<RwLock<HashMap<u32, WaitingEntry<TREQ, TRES>>>>,
    cron: Arc<Cron>,
    rto: Arc<RTO>,
    _0: PhantomData<TREQ>,
    _1: PhantomData<TRES>,
}

struct WaitingEntry<TREQ: Serializable, TRES: Serializable> {
    pub req: TREQ,
    pub req_bin: Arc<Vec<u8>>,
    pub ep: SocketAddr,
    pub callback: Box<Fn(TREQ, Option<TRES>)+Send+Sync>,
    pub transmit_time: u64,
    pub retransmit_count: AtomicUsize,
}

#[derive(RustcDecodable, RustcEncodable, Clone, Debug)]
enum InquiryMessage<TREQ: Serializable, TRES: Serializable> {
    Request(InquiryRequestMessage<TREQ>),
    Response(InquiryResponseMessage<TRES>),
}

#[derive(RustcDecodable, RustcEncodable, Clone, Debug)]
struct InquiryRequestMessage<T: Serializable> {
    pub id: u32,
    pub body: T,
}

#[derive(RustcDecodable, RustcEncodable, Clone, Debug)]
struct InquiryResponseMessage<T: Serializable> {
    pub id: u32,
    pub body: T,
}

impl<DS: 'static+DatagramSocket+Sync+Send, SERDE: 'static+SerDe, TREQ: 'static+Serializable, TRES: 'static+Serializable, RTO: 'static+RetransmissionTimerAlgorithm<IpAddr>> SimpleInquirySocket<DS, SERDE, TREQ, TRES, RTO> {
    pub fn new<H: 'static+Send+Fn(TREQ)->TRES>(sock: DS, serde: SERDE, handler: H, rto: RTO) -> SimpleInquirySocket<DS, SERDE, TREQ, TRES, RTO> {
        sock.set_blocking(false).unwrap();
        let flag = Arc::new(AtomicBool::new(false));
        let sock = Arc::new(sock);
        let serde = Arc::new(serde);
        let waiting_map = Arc::new(RwLock::new(HashMap::new()));
        let cron = Arc::new(Cron::new());
        let rto = Arc::new(rto);
        let thread_handle = {
            let flag = flag.clone();
            let sock = sock.clone();
            let serde = serde.clone();
            let waiting_map = waiting_map.clone();
            let rto = rto.clone();
            spawn(move || {
                Self::recv_thread(sock, serde, handler, flag, waiting_map, rto);
            })
        };
        let rng_seed = {
            let mut rng_seed = [0u32, 32];
            let mut rng = thread_rng();
            for i in 0..rng_seed.len() {
                rng_seed[i] = rng.next_u32();
            }
            rng_seed
        };
        SimpleInquirySocket {
            sock: sock,
            serde: serde,
            exit_flag: flag,
            recv_thread: Option::Some(thread_handle),
            reqid_rng: IsaacRng::from_seed(&rng_seed),
            waiting_map: waiting_map,
            cron: cron,
            rto: rto,
            _0: PhantomData,
            _1: PhantomData,
        }
    }

    /// 受信およびメッセージハンドラー実行スレッド
    fn recv_thread<H: Fn(TREQ)->TRES>(sock: Arc<DS>, serde: Arc<SERDE>, handler: H, flag: Arc<AtomicBool>,
                                      waiting_map: Arc<RwLock<HashMap<u32, WaitingEntry<TREQ, TRES>>>>,
                                      rto: Arc<RTO>) {
        let mut buf = [0u8; 2048]; // TODO: Max Datagram Size
        while !flag.load(Ordering::Relaxed) {
            let (size, remote_ep) = match sock.recv_from(&mut buf) {
                Ok((v0, v1)) => (v0, v1),
                Err(e) => {
                    if flag.load(Ordering::Relaxed) {
                        return;
                    }
                    if DS::is_would_block_err(&e) {
                        match sock.poll(100, SocketEventType::Read) {
                            _ => continue
                        }
                    } else {
                        break;
                    }
                },
            };
            let msg: InquiryMessage<TREQ, TRES> = match serde.deserialize(&buf[..size]) {
                Some(m) => m,
                _ => continue,
            };
            match msg {
                InquiryMessage::Request(req) => {
                    let res = handler(req.body);
                    let msg = serde.serialize(&InquiryMessage::Response::<TREQ, TRES>(InquiryResponseMessage {
                        id: req.id,
                        body: res,
                    })).unwrap();
                    Self::send_msg(&sock, &msg, &remote_ep, &flag).unwrap();
                },
                InquiryMessage::Response(res) => {
                    match waiting_map.write().unwrap().remove(&res.id) {
                        Some(entry) => {
                            let rtt = ((time::precise_time_ns() / 1000000) - entry.transmit_time) as u32;
                            rto.add_sample(&entry.ep.ip(), entry.transmit_time, rtt,
                                           entry.retransmit_count.load(Ordering::Relaxed) as u32);
                            (entry.callback)(entry.req, Option::Some(res.body));
                        },
                        _ => (),
                    }
                },
            }
        }
    }

    fn send_msg(sock: &DS, msg: &[u8], remote_ep: &SocketAddr, flag: &AtomicBool) -> Result<()>
    {
        while !flag.load(Ordering::Relaxed) {
            match sock.send_to(&msg, remote_ep) {
                Ok(_) => return Ok(()),
                Err(e) => {
                    if DS::is_would_block_err(&e) {
                        try!(sock.poll(-1, SocketEventType::Write));
                        continue;
                    } else {
                        return Err(e);
                    }
                }
            }
        }
        Ok(())
    }

    fn set_timeout(&self, id: u32) {
        let sock = self.sock.clone();
        let waiting_map = self.waiting_map.clone();
        let rto = self.rto.clone();
        let timeout = match waiting_map.read().unwrap().get(&id) {
            Some(entry) => rto.get_rto(&entry.ep.ip(), 0),
            _ => return,
        };
        self.cron.enqueue_dynamic_periodic_task(timeout, Box::new(move || -> u32 {
            let retransmit_info = {
                let locked = waiting_map.read().unwrap();
                let entry = match locked.get(&id) {
                    None => return 0,
                    Some(v) => v,
                };
                let retransmit_count = entry.retransmit_count.fetch_add(1, Ordering::SeqCst);
                if retransmit_count < 3 {
                    Some((entry.req_bin.clone(), entry.ep.clone(),
                          rto.get_rto(&entry.ep.ip(), 1 + retransmit_count as u32)))
                } else {
                    None
                }
            };
            match retransmit_info {
                Some((req, ep, rto)) => {
                    Self::send_msg(&sock, &req, &ep, &AtomicBool::new(false)).unwrap();
                    rto
                },
                _ => {
                    match waiting_map.write().unwrap().remove(&id) {
                        Some(entry) => {
                            (entry.callback)(entry.req, None);
                        },
                        _ => (),
                    }
                    0
                }
            }
        }));
    }

    pub fn raw_socket(&self) -> &DS {
        &self.sock
    }
}

impl<DS: DatagramSocket, SERDE: SerDe, TREQ: Serializable, TRES: Serializable, RTO: RetransmissionTimerAlgorithm<IpAddr>> Drop for SimpleInquirySocket<DS, SERDE, TREQ, TRES, RTO> {
    fn drop(&mut self) {
        self.exit_flag.store(true, Ordering::Relaxed);
        self.recv_thread.take().unwrap().join().unwrap();
    }
}

impl<DS: 'static+DatagramSocket+Sync+Send, SERDE: 'static+SerDe, TREQ: 'static+Serializable, TRES: 'static+Serializable, RTO: 'static+RetransmissionTimerAlgorithm<IpAddr>> InquirySocket<TREQ, TRES> for SimpleInquirySocket<DS, SERDE, TREQ, TRES, RTO> {
    fn inquire<CB>(&mut self, msg: TREQ, remote_ep: &SocketAddr, callback: CB) -> Result<()>
        where CB: 'static+Fn(TREQ, Option<TRES>)+Send+Sync
    {
        let (id, req) = {
            // ユニークなIDを乱数より生成ために，登録先Mapの排他ロックを獲得しておく
            let mut locked = self.waiting_map.write().unwrap();
            let mut id = self.reqid_rng.next_u32();
            while locked.contains_key(&id) {
                id = self.reqid_rng.next_u32();
            }

            let remote_ep = remote_ep.clone();
            let req = InquiryMessage::Request::<TREQ, TRES>(InquiryRequestMessage {
                id: id,
                body: msg,
            });
            let req_bin = Arc::new(self.serde.serialize(&req).unwrap());
            let entry = WaitingEntry {
                req: match req {
                    InquiryMessage::Request(x) => x.body,
                    _ => panic!(),
                },
                req_bin: req_bin.clone(),
                ep: remote_ep.clone(),
                callback: Box::new(callback),
                transmit_time: time::precise_time_ns() / 1000000,
                retransmit_count: AtomicUsize::new(0),
            };
            locked.insert(id, entry);
            (id, req_bin)
        };
        match Self::send_msg(&self.sock, &req, &remote_ep, &AtomicBool::new(false)) {
            Err(e) => {
                self.waiting_map.write().unwrap().remove(&id).unwrap();
                return Err(e);
            },
            _ => (),
        }

        self.set_timeout(id);
        Ok(())
    }
}

#[cfg(test)]
use std::net::{UdpSocket, SocketAddrV4, Ipv4Addr};

#[cfg(test)]
use std::str::FromStr;

#[cfg(test)]
use std::sync::Semaphore;

#[cfg(test)]
use ::io::{MsgPackSerDe};

#[test]
fn test_echo() {
    let mut sock0 = new_sock(test_echo_handler);
    let mut sock1 = new_sock(test_echo_handler);
    let sem0 = Arc::new(Semaphore::new(0));
    let sem1 = Arc::new(Semaphore::new(0));
    {
        let sem0 = sem0.clone();
        sock0.inquire("foo".to_string(), &sock1.raw_socket().local_addr().unwrap(), move |_, msg| {
            sem0.release();
            match msg {
                None => panic!("inquire failed #0"),
                Some(msg) => assert_eq!("echo: foo", &msg),
            }
        }).unwrap();

        let sem1 = sem1.clone();
        sock1.inquire("bar".to_string(), &sock0.raw_socket().local_addr().unwrap(), move |_, msg| {
            sem1.release();
            match msg {
                None => panic!("inquire failed #1"),
                Some(msg) => assert_eq!("echo: bar", &msg),
            }
        }).unwrap();
    }
    sem0.acquire();
    sem1.acquire();

    let sem0 = Arc::new(Semaphore::new(0));
    {
        let sem0 = sem0.clone();
        sock0.inquire("hoge".to_string(), &sock1.raw_socket().local_addr().unwrap(), move |_, msg| {
            sem0.release();
            match msg {
                None => panic!("inquire failed #2"),
                Some(msg) => assert_eq!("echo: hoge", &msg),
            }
        }).unwrap();
    }
    sem0.acquire();
}

#[test]
fn test_timeout() {
    let mut sock = new_sock(test_echo_handler);
    let sem = Arc::new(Semaphore::new(0));
    {
        let sem = sem.clone();
        let invalid_ep = {
            let ep = sock.raw_socket().local_addr().unwrap();
            SocketAddr::new(ep.ip(), ep.port() + 1)
        };
        sock.inquire("foo".to_string(), &invalid_ep, move |_, msg| {
            match msg {
                None => sem.release(),
                Some(_) => panic!("inquire failed"),
            }
        }).unwrap();
    }
    sem.acquire();
}

#[cfg(test)]
fn test_echo_handler(req: EchoReqT) -> EchoResT {
    let mut str = "echo: ".to_string();
    str.push_str(&req);
    str
}

#[cfg(test)]
type EchoReqT = String;

#[cfg(test)]
type EchoResT = String;

#[cfg(test)]
use super::RFC6298BasedRTO;

#[cfg(test)]
fn new_sock<H, TREQ, TRES>(handler: H) -> SimpleInquirySocket<UdpSocket, MsgPackSerDe, TREQ, TRES, RFC6298BasedRTO<IpAddr>>
    where H: 'static+Send+Fn(TREQ)->TRES, TREQ: 'static+Serializable, TRES: 'static+Serializable
{
    SimpleInquirySocket::new(
        UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::from_str("0.0.0.0").unwrap(), 0)).unwrap(),
        MsgPackSerDe,
        handler,
        RFC6298BasedRTO::new(1, 50),
    )
}
