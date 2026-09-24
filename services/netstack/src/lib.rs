extern crate alloc;

pub mod config;
pub mod dhcp;
pub mod dns;
pub mod ethernet;
pub mod link;
pub mod migration;
mod recovery;
pub mod router;
pub mod service;
pub mod smoltcp_runtime;
pub mod stack;
mod stream;
pub mod tcp;
pub mod udp;

pub use service::main;
