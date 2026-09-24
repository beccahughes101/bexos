extern crate alloc;

pub mod dns;
pub mod routing;

pub use service::main;

mod config;
mod migration;
mod service;
