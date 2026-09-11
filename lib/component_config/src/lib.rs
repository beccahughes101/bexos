//! Typed component configuration shared by assembly and runtime services.
#![no_std]
extern crate alloc;
pub mod schema;
pub mod storage;
mod table;
pub mod transaction;
pub mod wire;
pub use table::*;
