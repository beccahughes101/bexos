//! Versioned, bounded ABI shared by appd and the Starnix runner.
#![no_std]

extern crate alloc;

mod control;
mod launch;
mod options;
mod wire;

pub use control::Control;
pub use launch::Launch;
pub use options::{
    Environment, NixResourceLimit, NixRootFilesystem, NixRootSource, NixRunnerOptions,
};

pub const RUNNER_PACKAGE: &str = "bexos.platform.starnix_runner";
pub const RUNNER_PATH: &str = "/pkg/bin/starnix_runner";
pub const OPTIONS_TYPE_URL: &str = "type.googleapis.com/bexos.app.NixRunnerOptions";
pub const STARTUP_MAGIC: &[u8; 8] = b"BEXNIX01";
pub const CONTROL_MAGIC: &[u8; 8] = b"BEXCTL01";
pub const ABI_VERSION: u32 = 2;
pub const MAX_IMAGE_BYTES: u64 = 64 << 20;
pub const UPSTREAM_REVISION: &str = "cb5f36aee5510be9565392194f341d0409381db6";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    InvalidEncoding,
    InvalidOptions,
    LimitExceeded,
}
