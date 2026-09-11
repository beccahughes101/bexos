#![cfg_attr(not(feature = "std"), no_std)]
extern crate alloc;

pub mod guest;
pub use bexos_virtio_hal as hal;
pub mod hardware;
pub mod server;

pub mod pending;
