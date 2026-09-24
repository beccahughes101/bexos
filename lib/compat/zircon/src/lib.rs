//! Zircon-shaped ownership facade over BexOS userspace primitives.
#![no_std]

extern crate alloc;

mod channel;
mod handle;
mod status;
mod task;
mod time;
mod vmo;

pub use channel::{Channel, Socket};
pub use handle::{AsHandleRef, Handle, HandleRef, Rights};
pub use status::{Result, Status};
pub use task::{Futex, RestrictedState, Thread};
pub use time::{Clock, ClockId};
pub use vmo::{MappedVmo, Vmar, VmarFlags, Vmo};

pub const PAGE_SIZE: u64 = 4096;

pub fn system_get_page_size() -> u32 {
    PAGE_SIZE as u32
}
