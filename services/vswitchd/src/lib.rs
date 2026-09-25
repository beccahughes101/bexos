extern crate alloc;

mod builtins;
mod device;
mod extension_control;
pub mod extensions;
pub mod graph;
pub mod neighbor;
pub mod packet;
mod physical;
pub mod routing;
mod routing_control;
pub mod switch;

mod migration;
mod service;

pub use service::main;
