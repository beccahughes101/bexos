//! BexOS feature-gated root for the vendored upstream `starnix_kernel` crate.
//!
//! `fuchsia_main.rs` is the unmodified Fuchsia binary root at the pinned
//! revision. The BexOS target replaces component hosting with its trusted
//! runtime contract.
#![no_std]

#[path = "../../port/kernel.rs"]
mod bexos;

pub use bexos::*;
