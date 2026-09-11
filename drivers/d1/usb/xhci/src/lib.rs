#![cfg_attr(not(test), no_std)]
extern crate alloc;

mod hardware;
mod migration;
mod service;
mod wire;

pub use hardware::{Hardware, HardwareError};
pub use migration::Runtime;

pub async fn main(channel: u64) -> ! {
    service::main(channel).await
}
