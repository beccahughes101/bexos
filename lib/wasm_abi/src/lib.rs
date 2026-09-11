//! Versioned, bounded runner configuration shared by appd and the runtime.
#![no_std]
extern crate alloc;
pub mod options;
pub mod wire;
pub use options::{ComponentImport, Environment, Limits, WasmRunnerOptions};
pub const RUNNER_PACKAGE: &str = "bexos.platform.wasm_runner";
pub const RUNNER_PATH: &str = "/pkg/bin/wasm_runner";
pub const OPTIONS_TYPE_URL: &str = "type.googleapis.com/bexos.app.WasmRunnerOptions";
pub const STARTUP_MAGIC: &[u8; 8] = b"BEXWASM1";
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    InvalidEncoding,
    InvalidOptions,
    LimitExceeded,
}

pub mod launch;
pub use launch::ComponentDependency;
pub use launch::Launch;

pub mod signature;
