//! BexOS feature-gated root for the vendored upstream `starnix_core` crate.
//!
//! `fuchsia_lib.rs` is the unmodified Fuchsia root at the pinned revision.
//! BexOS selects the platform boundary below while keeping the upstream module
//! tree vendored for incremental replacement of compatibility gates.
#![no_std]

#[path = "../../../port/core.rs"]
mod bexos;

pub use bexos::*;
