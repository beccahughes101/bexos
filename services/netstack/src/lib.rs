extern crate alloc;

pub mod config;
pub mod dhcp;
pub mod dns;
pub mod ethernet;
pub mod link;
pub mod migration;
pub mod service;
pub mod smoltcp_runtime;
pub mod stack;
pub mod tcp;
pub mod udp;

pub use service::main;
