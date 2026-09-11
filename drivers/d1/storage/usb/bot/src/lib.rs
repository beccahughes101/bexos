#![cfg_attr(not(test), no_std)]
extern crate alloc;

pub mod block;
mod migration;
mod service;
mod transport;
mod wire;

pub use migration::Runtime;
pub use transport::{SenseData, TransportError, TransportState};

pub async fn main(channel: u64) -> ! {
    service::main(channel).await
}
