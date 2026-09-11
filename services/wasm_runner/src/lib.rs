//! Native process adapter. WASM code never receives process-control handles.
mod dispatch;
pub mod executor;
pub mod host;
pub mod launch;
pub mod migration;
pub mod service;

mod command;
mod composition;
