use super::RetransmissionTimerAlgorithm;
use std::collections::HashMap;
use std::cmp::{min, max};
use std::hash::Hash;
use std::sync::RwLock;

/// RFC6298をベースとした再送タイムアウト計算アルゴリズム
pub struct RFC6298BasedRTO<T: Hash + Eq> {
    state: RwLock<HashMap<T, State>>,
    min_rto: u32,
    clock_granularity: u32,
}

// RTOの初期値は1000[ms]
const DEFAULT_RTO: u32 = 1000;

// RTOの最大値は10秒 (RFC6298では60秒としてあるが，短めに設定)
const MAX_RTO: u32 = 10 * 1000;

struct State {
    srtt: i32,
    rttvar: i32,
}

impl<T: Sync + Send + Hash + Eq + Clone> RFC6298BasedRTO<T> {
    pub fn new(min_rto: u32, clock_granularity: u32) -> RFC6298BasedRTO<T> {
        RFC6298BasedRTO {
            state: RwLock::new(HashMap::new()),
            min_rto: min_rto,
            clock_granularity: clock_granularity,
        }
    }
}

impl<T: Sync + Send + Hash + Eq + Clone> RetransmissionTimerAlgorithm<T> for RFC6298BasedRTO<T> {
    fn add_sample(&self, key: &T, _: u64, rtt: u32, retransmit_cnt: u32) {
        if retransmit_cnt > 0 {
            return;
        }
        let mut locked = self.state.write().unwrap();
        match locked.get_mut(key) {
            Some(s) => {
                s.update(rtt);
                return;
            }
            _ => ()
        }
        locked.insert(key.clone(), State::new(rtt));
    }

    fn get_rto(&self, key: &T, retransmit_count: u32) -> u32 {
        let rto = match self.state.read().unwrap().get(key) {
            Some(s) => s.get_rto(self.clock_granularity, self.min_rto),
            _ => DEFAULT_RTO,
        };
        if retransmit_count == 0 {
            rto
        } else {
            rto * 2u32.pow(retransmit_count)
        }
    }
}

impl State {
    fn new(rtt: u32) -> State {
        State {
            srtt: (rtt * 8) as i32,
            rttvar: (rtt * 2) as i32,
        }
    }

    fn update(&mut self, rtt: u32) {
        let mut rtt = (rtt as i32) - (self.srtt >> 3);
        self.srtt += rtt;
        if rtt < 0 {
            rtt = -rtt;
        }
        self.rttvar -= (self.rttvar >> 2) - rtt;
    }

    fn get_rto(&self, clock_granularity: u32, min_rto: u32) -> u32 {
        let rto = (self.srtt >> 3) as u32 + max(clock_granularity, self.rttvar as u32);
        min(MAX_RTO, max(min_rto, rto))
    }
}

#[test]
fn test_rfc6298based() {
    let algo = RFC6298BasedRTO::<u32>::new(1, 50);
    assert_eq!(algo.get_rto(&0, 0), DEFAULT_RTO);
    algo.add_sample(&0, 0, 2000, 1);
    assert_eq!(algo.get_rto(&0, 0), DEFAULT_RTO); // Karn's algo
    algo.add_sample(&0, 0, 10, 0);
    assert_eq!(algo.get_rto(&0, 0), 60); // rtt + clock_granularity
    algo.add_sample(&0, 0, 20, 0);
    assert_eq!(algo.get_rto(&0, 0), 61); // rtt + clock_granularity
    algo.add_sample(&1, 0, 50, 0);
    assert_eq!(algo.get_rto(&1, 0), 150); // rtt + rttvar
    algo.add_sample(&1, 0, 100, 0);
    assert_eq!(algo.get_rto(&1, 0), 181); // rtt + rttvar

    assert_eq!(algo.get_rto(&2, 0), DEFAULT_RTO);
    assert_eq!(algo.get_rto(&2, 1), DEFAULT_RTO * 2);
    assert_eq!(algo.get_rto(&2, 2), DEFAULT_RTO * 4);
    assert_eq!(algo.get_rto(&2, 3), DEFAULT_RTO * 8);
    algo.add_sample(&2, 0, 10, 0);
    assert_eq!(algo.get_rto(&2, 0), 60);
    assert_eq!(algo.get_rto(&2, 1), 60 * 2);
    assert_eq!(algo.get_rto(&2, 2), 60 * 4);
    assert_eq!(algo.get_rto(&2, 3), 60 * 8);
}
