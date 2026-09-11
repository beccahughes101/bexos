//! Capability-independent Flatland scene, geometry and software composition.
#![no_std]
extern crate alloc;
pub mod accessibility;
mod blend;
pub mod blur;
pub mod composition;
pub mod effects;
pub mod kawase;
pub mod layout;
pub mod metrics;
pub mod presentation;
pub mod resolved;
pub mod scanout;
pub mod scene;
pub mod style;
pub mod surface;
pub mod synchronization;
pub mod world;
pub use surface::{Damage, Format, Surface};
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Invalid,
    Bounds,
    Stale,
    Busy,
    Denied,
}
