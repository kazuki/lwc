#![feature(slice_bytes)]
#![cfg_attr(test, feature(vec_resize))]

pub mod io;

#[cfg(not(test))]
fn main() {
    println!("Hello World");
}
