use std;
use std::collections::{BinaryHeap, HashMap};
use std::marker::{Sync, Send};
use std::sync::{Arc, Mutex, Condvar};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::{JoinHandle, spawn};
use time;

/// 任意の間隔をあけて，定期的または1度だけ実行するタスクを管理します
pub struct Cron {
    periodic_tasks: Arc<Mutex<HashMap<u32, Arc<PeriodicTask>>>>,
    events: Arc<Mutex<BinaryHeap<EventEntry>>>,
    thread: Option<JoinHandle<()>>,
    exit_flag: Arc<AtomicBool>,
    cond: Arc<Condvar>,
    periodic_task_seq: AtomicUsize,
}
unsafe impl Sync for Cron {}
unsafe impl Send for Cron {}

struct EventEntry {
    time: u64,
    task: Task,
}

enum Task {
    OneShot(Box<Fn()+Send+Sync>),
    Dynamic(Box<Fn()->u32+Send+Sync>),
    Periodical(Arc<PeriodicTask>),
}

impl std::cmp::PartialEq for EventEntry {
    fn eq(&self, other: &Self) -> bool {
        self.time.eq(&other.time)
    }
}
impl std::cmp::Eq for EventEntry {}
impl std::cmp::PartialOrd for EventEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        if self.time == other.time {
            Some(std::cmp::Ordering::Equal)
        } else if self.time < other.time {
            Some(std::cmp::Ordering::Greater)
        } else {
            Some(std::cmp::Ordering::Less)
        }
    }
}
impl std::cmp::Ord for EventEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.partial_cmp(other).unwrap()
    }
}

struct PeriodicTask {
    pub id: u32,
    pub func: Box<Fn()+Send+Sync>,
    pub interval: u32,
}

impl Cron {
    pub fn new() -> Cron {
        let tasks = Arc::new(Mutex::new(HashMap::new()));
        let events = Arc::new(Mutex::new(BinaryHeap::new()));
        let exit_flag = Arc::new(AtomicBool::new(false));
        let cond = Arc::new(Condvar::new());
        let thread_handle = {
            let tasks = tasks.clone();
            let events = events.clone();
            let exit_flag = exit_flag.clone();
            let cond = cond.clone();
            spawn(move || {
                Self::thread(tasks, events, exit_flag, cond);
            })
        };
        Cron {
            periodic_tasks: tasks,
            events: events,
            thread: Option::Some(thread_handle),
            exit_flag: exit_flag,
            cond: cond,
            periodic_task_seq: AtomicUsize::new(0),
        }
    }

    /// 1度だけ実行するタスクを登録します (スレッドセーフ)
    pub fn enqueue_oneshot(&self, interval: u32, func: Box<Fn()+Send+Sync>) {
        Self::enqueue(&self.events, &self.cond, interval, Task::OneShot(func));
    }

    /// タスクの処理結果に基づいて，同じタスクを再登録するようなタスクを登録します (スレッドセーフ)
    ///
    /// タスクの戻り値が 0 の場合は，タスクの再登録を行いません．
    pub fn enqueue_dynamic_periodic_task(&self, interval: u32, func: Box<Fn()->u32+Send+Sync>) {
        Self::enqueue(&self.events, &self.cond, interval, Task::Dynamic(func));
    }

    /// 任意の間隔で定期的に実行するタスクを登録します (スレッドセーフ)
    ///
    /// 戻り値としてタスクのIDを返却します．このIDを使ってタスクの登録を解除できます．
    pub fn register_periodic_task(&self, interval: u32, func: Box<Fn()+Send+Sync>) -> u32 {
        let id = self.periodic_task_seq.fetch_add(1, Ordering::SeqCst) as u32;
        let task = Arc::new(PeriodicTask {
            id: id,
            func: func,
            interval: interval,
        });
        Self::enqueue(&self.events, &self.cond, interval, Task::Periodical(task.clone()));
        self.periodic_tasks.lock().unwrap().insert(id, task);
        id
    }

    /// 定期的に実行するタスクの登録を解除します (スレッドセーフ)
    pub fn unregister_periodic_task(&self, id: u32) -> bool {
        match self.periodic_tasks.lock().unwrap().remove(&id) {
            Some(_) => true,
            _ => false,
        }
    }

    fn enqueue(events: &Arc<Mutex<BinaryHeap<EventEntry>>>, cond: &Arc<Condvar>,
               interval: u32, task: Task) {
        let cur = time::precise_time_ns() / 1000000; // [ms]
        let mut events = events.lock().unwrap();
        events.push(EventEntry {
            time: cur + interval as u64,
            task: task
        });
        cond.notify_all();
    }

    fn thread(tasks: Arc<Mutex<HashMap<u32, Arc<PeriodicTask>>>>, events: Arc<Mutex<BinaryHeap<EventEntry>>>,
              exit_flag: Arc<AtomicBool>, cond: Arc<Condvar>) {
        while !exit_flag.load(Ordering::Relaxed) {
            {
                let events = events.lock().unwrap();
                let sleep_ms = match events.peek() {
                    Some(entry) => {
                        let t = (entry.time as i64) - (time::precise_time_ns() / 1000000) as i64;
                        std::cmp::max(0, std::cmp::min(t, 60000))
                    },
                    _ => -1
                };
                if sleep_ms < 0 {
                    match cond.wait(events) { _ => () } // unused_variable対策
                } else if sleep_ms > 0 {
                    match cond.wait_timeout_ms(events, sleep_ms as u32) { _ => () } // // unused_variable対策
                }
            }

            while !exit_flag.load(Ordering::Relaxed) {
                let task = {
                    let mut events = events.lock().unwrap();
                    match events.peek() {
                        Some(entry) if entry.time <= (time::precise_time_ns() / 1000000) => (),
                        _ => break,
                    };
                    events.pop().unwrap().task
                };

                match task {
                    Task::OneShot(f) => f(),
                    Task::Dynamic(f) => {
                        let next_interval = f();
                        if next_interval > 0 {
                            Self::enqueue(&events, &cond, next_interval, Task::Dynamic(f));
                        }
                    },
                    Task::Periodical(pt) => {
                        (pt.func)();
                        if tasks.lock().unwrap().contains_key(&pt.id) {
                            Self::enqueue(&events, &cond, pt.interval, Task::Periodical(pt));
                        }
                    },
                }
            }
        }
    }
}

impl Drop for Cron {
    fn drop(&mut self) {
        {
            let mut guard = self.events.lock().unwrap();
            guard.clear(); // クリアしなくても良いがunused_variable警告解除のため
            self.exit_flag.store(true, Ordering::SeqCst);
            self.cond.notify_all();
        }
        self.thread.take().unwrap().join().unwrap();
    }
}
