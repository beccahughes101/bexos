//! Shared terminal control and bounded line discipline for terminal providers.
extern crate alloc;
pub mod discipline;
#[cfg(not(target_family = "wasm"))]
pub mod frontend;
#[cfg(not(target_family = "wasm"))]
pub use frontend::Frontend;
pub use tty_fidl::{Signal, TerminalMode, WindowSize};

pub mod provider;

pub mod transport;
