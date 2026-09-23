//! Shared, capability-oriented WebAssembly execution for BexOS.
#![cfg_attr(bexos_guest, feature(thread_local))]
pub mod engine;
mod platform;

mod abi;
pub mod budget;
pub mod context;
pub mod host;
pub mod resources;

pub mod control;
pub mod instance;

pub mod wasi;

mod sandbox;

pub mod bindings;
pub mod child;
pub mod component;
pub mod component_host;
mod component_locale;
pub mod component_sandbox;
pub mod lifecycle;
pub mod migration;
pub mod service_component;
pub mod service_guest;
#[cfg(test)]
mod tests;

mod admission;

pub mod prepared;

mod warm_restore;

mod terminal_host;
