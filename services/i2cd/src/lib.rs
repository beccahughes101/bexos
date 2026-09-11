#![cfg_attr(not(test), no_std)]
extern crate alloc;

mod migration;
mod registry;
mod service;
mod wire;

pub use migration::Runtime;

pub async fn main(channel: u64) -> ! {
    service::main(channel).await
}
