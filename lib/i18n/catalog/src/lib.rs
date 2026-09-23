//! Versioned, borrowed Fluent bytecode. No FTL parser is linked into clients.
#![no_std]
extern crate alloc;
pub mod builder;
mod format;
mod resolver;
pub use format::{Catalog, Error, Node, kind};
pub use resolver::{Arguments, Formatter, Resolver, Value};
