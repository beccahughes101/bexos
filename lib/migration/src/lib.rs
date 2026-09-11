//! Shared, transport-independent live migration primitives.
#![no_std]
extern crate alloc;

pub mod codec;
pub mod dirty;
pub mod records;
pub mod session;

pub use session::{Error, Phase, Session, Timeouts};

pub const VERSION: u64 = 1;
pub const DEFAULT_CUTOVER_MS: u64 = 150;
pub const DEFAULT_PREPARATION_MS: u64 = 30_000;
pub const MAX_CHUNK_BYTES: usize = 32 * 1024;

pub mod blob;
