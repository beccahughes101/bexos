extern crate alloc;

use alloc::vec::Vec;
use bexos_userspace::{Channel, Message, Rpc};
use kernel_fidl::Status;

pub const DEFAULT_RECV_SPINS: u64 = 60_000;

pub async fn yield_once() {
    bexos_userspace::yield_now();
}

pub async fn recv(channel: Channel) -> Result<Message, Status> {
    recv_with_spin_limit(channel, DEFAULT_RECV_SPINS).await
}

pub async fn recv_with_spin_limit(channel: Channel, max_spins: u64) -> Result<Message, Status> {
    let mut spins = 0;
    loop {
        match channel.try_recv() {
            Err(Status::ErrTimedOut) if spins < max_spins => {
                spins += 1;
                yield_once().await;
            }
            result => return result,
        }
    }
}

pub async fn send(channel: Channel, bytes: &[u8], handles: &[u64]) -> Result<(), Status> {
    channel.send(bytes, handles)
}

pub async fn call_raw(
    rpc: Rpc,
    ordinal: u64,
    request: &[u8],
    handles: &[u64],
    reply: bool,
) -> Result<Message, Status> {
    let mut bytes = Vec::with_capacity(8 + request.len());
    bytes.extend_from_slice(&ordinal.to_le_bytes());
    bytes.extend_from_slice(request);
    rpc.0.send(&bytes, handles)?;
    if reply {
        recv(rpc.0).await
    } else {
        Ok(Message {
            bytes: Vec::new(),
            handles: Vec::new(),
        })
    }
}
