extern crate alloc;

pub mod config;
pub mod migration;
pub mod nts;
pub mod service;
pub mod sntp;
pub mod state;

pub use service::main;
