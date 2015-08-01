#![feature(slice_bytes, ip_addr)]
#![cfg_attr(test, feature(semaphore, vec_resize))]

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
