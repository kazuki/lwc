use super::{DatagramSocket, InquirySocket, Serializable, SocketEventType};
use ::thread::{Cron};
use ::io::{SerDe};
use std::collections::HashMap;
use std::net::{ToSocketAddrs};
use std::io::{Error, ErrorKind, Result};
use std::fmt::Debug;
use std::marker::{Sync, Send, PhantomData};
use std::sync::{Arc, RwLock};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::{JoinHandle, spawn};
use rustc_serialize::{Encoder, Encodable, Decoder};
use rand::{IsaacRng, Rng, SeedableRng, thread_rng};

/// データグラム上で`InquirySocket`を実現する
pub struct SimpleInquirySocket<DS: DatagramSocket, SERDE: SerDe, TREQ: Serializable, TRES: Serializable> {
    sock: Arc<DS>,
    serde: Arc<SERDE>,
    exit_flag: Arc<AtomicBool>,
    recv_thread: Option<JoinHandle<()>>,
    reqid_rng: IsaacRng,
    waiting_map: Arc<RwLock<HashMap<u32, WaitingEntry<TREQ, TRES>>>>,
    cron: Arc<Cron>,
    _0: PhantomData<TREQ>,
    _1: PhantomData<TRES>,
}

struct WaitingEntry<TREQ: Serializable, TRES: Serializable> {
    pub req: InquiryMessage<TREQ, TRES>,
    pub callback: Box<Fn(Option<TRES>)+Send+Sync>,
    pub retransmit: Box<Fn(&InquiryMessage<TREQ, TRES>)+Send+Sync>,
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

impl<DS: 'static+DatagramSocket+Sync+Send, SERDE: 'static+SerDe, TREQ: 'static+Serializable, TRES: 'static+Serializable> SimpleInquirySocket<DS, SERDE, TREQ, TRES> {
    pub fn new<H: 'static+Send+Fn(TREQ)->TRES>(sock: DS, serde: SERDE, handler: H) -> SimpleInquirySocket<DS, SERDE, TREQ, TRES> {
        sock.set_blocking(false).unwrap();
        let flag = Arc::new(AtomicBool::new(false));
        let sock = Arc::new(sock);
        let serde = Arc::new(serde);
        let waiting_map = Arc::new(RwLock::new(HashMap::new()));
        let cron = Arc::new(Cron::new());
        let thread_handle = {
            let flag = flag.clone();
            let sock = sock.clone();
            let serde = serde.clone();
            let waiting_map = waiting_map.clone();
            spawn(move || {
                Self::recv_thread(sock, serde, handler, flag, waiting_map);
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
            _0: PhantomData,
            _1: PhantomData,
        }
    }

    /// 受信およびメッセージハンドラー実行スレッド
    fn recv_thread<H: Fn(TREQ)->TRES>(sock: Arc<DS>, serde: Arc<SERDE>, handler: H, flag: Arc<AtomicBool>,
                                      waiting_map: Arc<RwLock<HashMap<u32, WaitingEntry<TREQ, TRES>>>>) {
        let mut buf = [0u8; 2048]; // TODO: Max Datagram Size
        let mut panic_flag = false;
        while !panic_flag && !flag.load(Ordering::Relaxed) {
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
            let ret = match msg {
                InquiryMessage::Request(req) => {
                    let res = handler(req.body);
                    let msg = InquiryMessage::Response::<TREQ, TRES>(InquiryResponseMessage {
                        id: req.id,
                        body: res,
                    });
                    Self::send_msg(&sock, &serde, &msg, &remote_ep, &flag)
                },
                InquiryMessage::Response(res) => {
                    match waiting_map.write().unwrap().remove(&res.id) {
                        Some(entry) => {
                            (entry.callback)(Option::Some(res.body));
                        },
                        _ => (),
                    }
                    Ok(())
                },
            };
            match ret {
                Ok(_) => (),
                Err(_) => {
                    panic_flag = true;
                    break;
                }
            }
        }
        if panic_flag {
            panic!("thread exit from unknown reason");
        }
    }

    fn send_msg<T, A>(sock: &DS, serde: &SERDE, msg: &T, remote_ep: &A, flag: &AtomicBool) -> Result<()>
        where T: Serializable, A: ToSocketAddrs+Debug
    {
        // TODO: 再送時にシリアライザが再度走るのもムダなので，バイト列を受け取るようにする
        let msg_bin = match serde.serialize(msg) {
            Some(m) => m,
            _ => return Err(Error::new(ErrorKind::InvalidInput, "serialization error"))
        };
        while !flag.load(Ordering::Relaxed) {
            match sock.send_to(&msg_bin, remote_ep) {
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
        let waiting_map = self.waiting_map.clone();
        self.cron.enqueue_dynamic_periodic_task(1000, Box::new(move || -> u32 {
            {
                let locked = waiting_map.read().unwrap();
                let entry = match locked.get(&id) {
                    None => return 0,
                    Some(v) => v,
                };
                if entry.retransmit_count.fetch_add(1, Ordering::SeqCst) < 3 {
                    // lifetimeの都合上，共有ロック内でsemd_msgしている..
                    (entry.retransmit)(&entry.req);
                    return 1000;
                }
            }

            match waiting_map.write().unwrap().remove(&id) {
                Some(entry) => {
                    (entry.callback)(None);
                },
                _ => (),
            }
            0
        }));
    }

    pub fn raw_socket(&self) -> &DS {
        &self.sock
    }
}

impl<DS: DatagramSocket, SERDE: SerDe, TREQ: Serializable, TRES: Serializable> Drop for SimpleInquirySocket<DS, SERDE, TREQ, TRES> {
    fn drop(&mut self) {
        self.exit_flag.store(true, Ordering::Relaxed);
        self.recv_thread.take().unwrap().join().unwrap();
    }
}

impl<DS: 'static+DatagramSocket+Sync+Send, SERDE: 'static+SerDe, TREQ: 'static+Serializable, TRES: 'static+Serializable> InquirySocket<TREQ, TRES> for SimpleInquirySocket<DS, SERDE, TREQ, TRES> {
    fn inquire<EP, CB>(&mut self, msg: TREQ, remote_ep: EP, callback: CB) -> Result<()>
        where EP: 'static+ToSocketAddrs+Clone+Send+Sync+Debug, CB: 'static+Fn(Option<TRES>)+Send+Sync
    {
        let (id, req) = {
            // ユニークなIDを乱数より生成ために，登録先Mapの排他ロックを獲得しておく
            let mut locked = self.waiting_map.write().unwrap();
            let mut id = self.reqid_rng.next_u32();
            while locked.contains_key(&id) {
                id = self.reqid_rng.next_u32();
            }

            let sock = self.sock.clone();
            let serde = self.serde.clone();
            let remote_ep = remote_ep.clone();
            let entry = WaitingEntry {
                req: InquiryMessage::Request::<TREQ, TRES>(InquiryRequestMessage {
                    id: id,
                    body: msg,
                }),
                callback: Box::new(callback),
                retransmit: Box::new(move |msg| {
                    Self::send_msg(&sock, &serde, msg, &remote_ep, &AtomicBool::new(false)).unwrap();
                }),
                retransmit_count: AtomicUsize::new(0),
            };
            let req = entry.req.clone(); // lifetimeの都合上，cloneする以外の方法が思いつかなかった(ロック外でsend_msgを呼び出したいので...)
            locked.insert(id, entry);
            (id, req)
        };
        match Self::send_msg(&self.sock, &self.serde, &req, &remote_ep, &AtomicBool::new(false)) {
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
use std::net::{UdpSocket, SocketAddr, SocketAddrV4, Ipv4Addr};

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
        sock0.inquire("foo".to_string(), sock1.raw_socket().local_addr().unwrap(), move |msg| {
            sem0.release();
            match msg {
                None => panic!("inquire failed #0"),
                Some(msg) => assert_eq!("echo: foo", &msg),
            }
        }).unwrap();

        let sem1 = sem1.clone();
        sock1.inquire("bar".to_string(), sock0.raw_socket().local_addr().unwrap(), move |msg| {
            sem1.release();
            match msg {
                None => panic!("inquire failed #1"),
                Some(msg) => assert_eq!("echo: bar", &msg),
            }
        }).unwrap();
    }
    sem0.acquire();
    sem1.acquire();
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
        sock.inquire("foo".to_string(), invalid_ep, move |msg| {
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
fn new_sock<H, TREQ, TRES>(handler: H) -> SimpleInquirySocket<UdpSocket, MsgPackSerDe, TREQ, TRES>
    where H: 'static+Send+Fn(TREQ)->TRES, TREQ: 'static+Serializable, TRES: 'static+Serializable
{
    SimpleInquirySocket::new(
        UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::from_str("0.0.0.0").unwrap(), 0)).unwrap(),
        MsgPackSerDe,
        handler
    )
}
