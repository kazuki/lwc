use super::{DatagramSocket, InquirySocket, Serializable, SocketEventType, RetransmissionTimerAlgorithm};
use ::thread::{Cron};
use ::io::{SerDe};
use std::any::{Any, TypeId};
use std::boxed::FnBox;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::io::{Error, ErrorKind, Result};
use std::marker::{Sync, Send};
use std::sync::{Arc, RwLock, Mutex};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::{JoinHandle, spawn};
use rand::{IsaacRng, Rng, SeedableRng, thread_rng};
use time;

/// データグラム上で`InquirySocket`を実現する
pub struct SimpleInquirySocket<DS: DatagramSocket, SERDE: SerDe, RTO: RetransmissionTimerAlgorithm<IpAddr>> {
    sock: Arc<DS>,
    serde: Arc<SERDE>,
    exit_flag: Arc<AtomicBool>,
    threads: Option<Vec<JoinHandle<()>>>,
    reqid_rng: Mutex<IsaacRng>,
    waiting_map: Arc<RwLock<HashMap<u32, WaitingEntry>>>,
    handlers: Arc<RwLock<HashMap<u32, HandlerEntry>>>,
    handler_types: Arc<RwLock<HashMap<TypeId, (u32, TypeId)>>>,
    cron: Arc<Cron>,
    rto: Arc<RTO>,
}

struct HandlerEntry {
    pub handler: Box<Fn(u32,&[u8])->Option<Vec<u8>>+Send+Sync>,
    pub req_type: TypeId,
    pub res_type: TypeId,
}

struct WaitingEntry {
    pub ep: SocketAddr,
    pub req_bin: Arc<Vec<u8>>,
    pub callback: Box<FnBox(&[u8])+Send+Sync>,
    pub transmit_time: u64,
    pub retransmit_count: AtomicUsize,
}

impl<DS: 'static+DatagramSocket, SERDE: 'static+SerDe, RTO: 'static+RetransmissionTimerAlgorithm<IpAddr>> SimpleInquirySocket<DS, SERDE, RTO> {
    pub fn new(sock: DS, serde: SERDE, rto: RTO, num_threads: usize) -> SimpleInquirySocket<DS, SERDE, RTO> {
        sock.set_blocking(false).unwrap();
        let flag = Arc::new(AtomicBool::new(false));
        let sock = Arc::new(sock);
        let serde = Arc::new(serde);
        let waiting_map = Arc::new(RwLock::new(HashMap::new()));
        let handlers = Arc::new(RwLock::new(HashMap::new()));
        let handler_types = Arc::new(RwLock::new(HashMap::new()));
        let cron = Arc::new(Cron::new());
        let rto = Arc::new(rto);
        let mut threads = Vec::new();
        for _ in 0..num_threads {
            threads.push({
                let flag = flag.clone();
                let sock = sock.clone();
                let waiting_map = waiting_map.clone();
                let handlers = handlers.clone();
                let rto = rto.clone();
                spawn(move || {
                    Self::recv_thread(sock, flag, waiting_map, handlers, rto);
                })
            });
        }
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
            threads: Option::Some(threads),
            reqid_rng: Mutex::new(IsaacRng::from_seed(&rng_seed)),
            waiting_map: waiting_map,
            handlers: handlers,
            handler_types: handler_types,
            cron: cron,
            rto: rto,
        }
    }

    fn recv_thread(sock: Arc<DS>, exit_flag: Arc<AtomicBool>,
                   waiting_map: Arc<RwLock<HashMap<u32, WaitingEntry>>>,
                   handlers: Arc<RwLock<HashMap<u32, HandlerEntry>>>, rto: Arc<RTO>) {
        let mut buf = [0u8; 2048]; // TODO: Max Datagram Size
        while !exit_flag.load(Ordering::Relaxed) {
            let (size, remote_ep) = match sock.recv_from(&mut buf) {
                Ok((v0, v1)) => (v0, v1),
                Err(e) => {
                    if exit_flag.load(Ordering::Relaxed) {
                        return;
                    } else if DS::is_would_block_err(&e) {
                        match sock.poll(100, SocketEventType::Read) {
                            _ => continue
                        }
                    } else {
                        panic!();
                    }
                },
            };

            // IDとリクエスト・レスポンスを識別するフラグをパースする
            if size < 4 {
                continue;
            }
            let (id, is_req) = {
                let x = (buf[0] as u32) | ((buf[1] as u32) << 8) | ((buf[2] as u32) << 16) | ((buf[3] as u32) << 24);
                (x & 0x7fffffff, (x >> 31) == 0)
            };

            if is_req {
                if size < 8 {
                    continue;
                }
                let type_id = (buf[4] as u32) | ((buf[5] as u32) << 8) | ((buf[6] as u32) << 16) | ((buf[7] as u32) << 24);
                let res = {
                    match handlers.read().unwrap().get(&type_id) {
                        Some(v) => (v.handler)(id | 0x80000000, &buf[8..size]),
                        None => None,
                    }
                };

                match res {
                    Some(res) => {
                        Self::send_msg(&sock, &res, &remote_ep, &exit_flag).unwrap();
                    },
                    None => (),
                }
            } else {
                match waiting_map.write().unwrap().remove(&id) {
                    Some(entry) => {
                        let rtt = ((time::precise_time_ns() / 1000000) - entry.transmit_time) as u32;
                        rto.add_sample(&entry.ep.ip(), entry.transmit_time, rtt,
                                       entry.retransmit_count.load(Ordering::Relaxed) as u32);
                        entry.callback.call_box((&buf[4..size],));
                    },
                    _ => (),
                }
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
                            entry.callback.call_box((&[0u8;0],));
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

impl<DS: 'static+DatagramSocket, SERDE: 'static+SerDe, RTO: 'static+RetransmissionTimerAlgorithm<IpAddr>> InquirySocket for SimpleInquirySocket<DS, SERDE, RTO> {
    fn register_handler<T, TREQ, TRES>(&self, id: u32, handler: T) -> bool
        where TREQ: Serializable, TRES: Serializable, T: 'static+Send+Sync+Fn(TREQ)->TRES
    {
        let req_type = TypeId::of::<TREQ>();
        let res_type = TypeId::of::<TRES>();
        {
            let mut locked = self.handler_types.write().unwrap();
            if locked.contains_key(&req_type) {
                return false;
            }
            locked.insert(req_type.clone(),
                          (id.clone(), res_type.clone()));
        }

        let serde = self.serde.clone();
        let handler = move |id: u32, binary: &[u8]| {
            let req = match serde.deserialize(binary) {
                None => return None,
                Some(x) => x,
            };
            let mut buf = vec![
                (id & 0xff) as u8,
                ((id >> 8) & 0xff) as u8,
                ((id >> 16) & 0xff) as u8,
                ((id >> 24) & 0xff) as u8
            ];
            match serde.serialize(&mut buf, &handler(req)) {
                Ok(_) => return Some(buf),
                _ => return None,
            }
        };
        let mut locked = self.handlers.write().unwrap();
        if locked.contains_key(&id) {
            self.handler_types.write().unwrap().remove(&req_type);
            return false;
        }
        locked.insert(id, HandlerEntry {
            handler: Box::new(handler),
            req_type: req_type,
            res_type: res_type,
        });
        return true;
    }

    fn inquire<TREQ, TRES, CB>(&self, msg: TREQ, remote_ep: &SocketAddr, callback: CB) -> Result<()>
        where TREQ: Serializable, TRES: Serializable, CB: 'static+Fn(TREQ, Option<TRES>)+Send+Sync+Any
    {
        let type_id = match self.handler_types.read().unwrap().get(&TypeId::of::<TREQ>()) {
            Some(&(type_id, _)) => type_id,
            _ => return Err(Error::new(ErrorKind::InvalidInput, "unregistered request type")),
        };

        let (id, req) = {
            // ユニークなIDを乱数より生成ために，登録先Mapの排他ロックを獲得しておく
            let mut locked = self.waiting_map.write().unwrap();
            let id = {
                let mut rng = self.reqid_rng.lock().unwrap();
                let mut id = rng.next_u32() & 0x7fffffff;
                while locked.contains_key(&id) {
                    id = rng.next_u32() & 0x7fffffff;
                }
                id
            };

            let remote_ep = remote_ep.clone();
            let req_bin = Arc::new({
                let mut buf = vec![
                    (id & 0xff) as u8,
                    ((id >> 8) & 0xff) as u8,
                    ((id >> 16) & 0xff) as u8,
                    ((id >> 24) & 0xff) as u8,
                    (type_id & 0xff) as u8,
                    ((type_id >> 8) & 0xff) as u8,
                    ((type_id >> 16) & 0xff) as u8,
                    ((type_id >> 24) & 0xff) as u8,
                ];
                self.serde.serialize(&mut buf, &msg).unwrap();
                buf
            });
            let serde = self.serde.clone();
            let callback_wrapper: Box<FnBox(&[u8])+Send+Sync> = Box::new(move |binary: &[u8]| {
                if binary.len() == 0 {
                    callback(msg, Option::None);
                } else {
                    let res: TRES = serde.deserialize(binary).unwrap();
                    callback(msg, Option::Some(res));
                }
            });
            let entry = WaitingEntry {
                req_bin: req_bin.clone(),
                ep: remote_ep.clone(),
                callback: callback_wrapper,
                transmit_time: time::precise_time_ns() / 1000000,
                retransmit_count: AtomicUsize::new(0),
            };
            locked.insert(id, entry);
            (id, req_bin)
        };
        match Self::send_msg(&self.sock, &req, &remote_ep, &self.exit_flag) {
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

impl<DS: DatagramSocket, SERDE: SerDe, RTO: RetransmissionTimerAlgorithm<IpAddr>> Drop for SimpleInquirySocket<DS, SERDE, RTO> {
    fn drop(&mut self) {
        self.exit_flag.store(true, Ordering::SeqCst);
        for thread in self.threads.take().unwrap() {
            thread.join().unwrap();
        }
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

#[cfg(test)]
use ::net::RFC6298BasedRTO;

#[cfg(test)]
type EchoReqT = String;

#[cfg(test)]
type EchoResT = String;

#[test]
fn test_echo() {
    let sock0 = new_sock();
    let sock1 = new_sock();
    let sem0 = Arc::new(Semaphore::new(0));
    let sem1 = Arc::new(Semaphore::new(0));
    {
        let sem0 = sem0.clone();
        sock0.inquire("foo".to_string(), &sock1.raw_socket().local_addr().unwrap(), move |_: EchoReqT, msg: Option<EchoResT>| {
            sem0.release();
            match msg {
                None => panic!("inquire failed #0"),
                Some(msg) => assert_eq!("echo: foo", &msg),
            }
        }).unwrap();

        let sem1 = sem1.clone();
        sock1.inquire("bar".to_string(), &sock0.raw_socket().local_addr().unwrap(), move |_: EchoReqT, msg: Option<EchoResT>| {
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
        sock0.inquire("hoge".to_string(), &sock1.raw_socket().local_addr().unwrap(), move |_: EchoReqT, msg: Option<EchoResT>| {
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
    let sock = new_sock();
    let sem = Arc::new(Semaphore::new(0));
    {
        let sem = sem.clone();
        let invalid_ep = {
            let ep = sock.raw_socket().local_addr().unwrap();
            SocketAddr::new(ep.ip(), ep.port() + 1)
        };
        sock.inquire("foo".to_string(), &invalid_ep, move |_: EchoReqT, msg: Option<EchoResT>| {
            match msg {
                None => sem.release(),
                Some(_) => panic!("inquire failed"),
            }
        }).unwrap();
    }
    sem.acquire();
}

#[cfg(test)]
fn new_sock() -> SimpleInquirySocket<UdpSocket, MsgPackSerDe, RFC6298BasedRTO<IpAddr>> {
    let sock = SimpleInquirySocket::new(
        UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::from_str("0.0.0.0").unwrap(), 0)).unwrap(),
        MsgPackSerDe,
        RFC6298BasedRTO::new(1, 50),
        2,
    );
    sock.register_handler(0, move |req: EchoReqT| {
        let mut str = "echo: ".to_string();
        str.push_str(&req);
        str
    });
    sock
}
