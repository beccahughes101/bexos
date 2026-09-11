extern crate alloc;

pub mod migration;
pub mod runtime;
pub mod service;
pub mod storage;
pub mod wire;

pub use runtime::main;
