#![cfg_attr(not(test), no_std)]
extern crate alloc;

mod migration;
mod parser;
mod service;
mod wire;

pub use migration::{HidKind, Runtime};
pub use parser::{ReportBatch, report_events};
pub use service::{deliver_keyboard, deliver_mouse};

pub async fn main(channel: u64) -> ! {
    service::main(channel).await
}
