#![no_std]

extern crate alloc;

#[cfg(feature = "guest")]
pub mod guest;
pub mod model;

pub use model::{ArchiveFs, ArchiveFsError, FileHandle, NodeAttributes, NodeKind, OpenNode};
