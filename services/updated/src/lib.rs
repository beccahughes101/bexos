#![allow(async_fn_in_trait)]

extern crate alloc;

pub mod migration;
pub mod service;
pub mod wire;

pub use service::main;
