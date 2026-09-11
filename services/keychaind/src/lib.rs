extern crate alloc;
mod clients;

pub mod migration;
pub mod runtime;
pub mod service;
pub mod wire;

pub use runtime::main;
