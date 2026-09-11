//! Transport-independent PTY ownership. WASM implementations use local resource IDs.
use alloc::vec::Vec;
use kernel_fidl::Status;

pub struct Message {
    pub bytes: Vec<u8>,
    pub handles: Vec<u64>,
}

/// Successful sends transfer handles; failed sends retain ownership in the caller.
pub trait Transport {
    fn socket_pair(&self) -> Result<(u64, u64), Status>;
    fn receive(&self, channel: u64) -> Result<Message, Status>;
    fn send(&self, channel: u64, bytes: &[u8], handles: &[u64]) -> Result<(), Status>;
    fn close(&self, handle: u64) -> Result<(), Status>;
}

#[cfg(not(target_family = "wasm"))]
pub struct NativeTransport;
#[cfg(not(target_family = "wasm"))]
impl Transport for NativeTransport {
    fn socket_pair(&self) -> Result<(u64, u64), Status> {
        bexos_userspace::Socket::pair().map(|(a, b)| (a.0, b.0))
    }
    fn receive(&self, channel: u64) -> Result<Message, Status> {
        bexos_userspace::Channel(channel)
            .try_recv()
            .map(|m| Message {
                bytes: m.bytes,
                handles: m.handles,
            })
    }
    fn send(&self, channel: u64, bytes: &[u8], handles: &[u64]) -> Result<(), Status> {
        bexos_userspace::Channel(channel).send(bytes, handles)
    }
    fn close(&self, handle: u64) -> Result<(), Status> {
        bexos_userspace::Memory::close(handle)
    }
}
