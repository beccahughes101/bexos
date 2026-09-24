#![no_std]

extern crate alloc;

mod device;
mod physical;
pub mod switch;

mod migration;
mod service;

pub use service::main;
