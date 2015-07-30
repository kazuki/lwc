#![feature(slice_bytes)]
#![cfg_attr(test, feature(semaphore))]
#![cfg_attr(test, feature(vec_resize))]
#![cfg_attr(test, feature(ip_addr))]

extern crate rustc_serialize;
extern crate rmp;
extern crate rand;
extern crate time;

pub mod net;
pub mod io;
pub mod thread;

#[cfg(not(test))]
fn main() {
    println!("Hello World");
}
