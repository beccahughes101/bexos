#![cfg_attr(not(feature = "guest"), no_std)]
extern crate alloc;

mod model;
pub use model::{
    DirectoryEntry, MemFs, MemFsError, NodeAttributes, NodeKind, OpenedNode, ROOT_INODE,
};

#[cfg(feature = "guest")]
pub mod guest;
