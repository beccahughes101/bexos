#![no_std]

extern crate alloc;

pub mod controller;
pub mod integration;
pub mod migration;

pub use controller::{
    ConnectionGuard, IdleDecision, KeepAlive, LazyServiceController, LazyServiceSnapshot,
    StopDecision,
};
pub use integration::{BindingAcceptError, GuardedServiceEndpoint, accept_control_binding};
