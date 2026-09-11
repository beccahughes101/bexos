//! Native-independent transport adapter for the shared terminal provider.
use crate::bexos::wasm::kernel;
use bexos_tty::transport::{Message, Transport};
use kernel_fidl::Status;

pub struct WasmTransport;
fn id(raw: u64) -> Result<u32, Status> {
    u32::try_from(raw)
        .ok()
        .filter(|id| *id != 0)
        .ok_or(Status::ErrInvalidHandle)
}
impl Transport for WasmTransport {
    fn socket_pair(&self) -> Result<(u64, u64), Status> {
        kernel::socket_pair()
            .map(|(a, b)| (a.into(), b.into()))
            .map_err(|_| Status::ErrResourceExhausted)
    }
    fn receive(&self, channel: u64) -> Result<Message, Status> {
        kernel::channel_read_checked(id(channel)?, 32768, 16)
            .map(|m| Message {
                bytes: m.data,
                handles: m.resources.into_iter().map(u64::from).collect(),
            })
            .map_err(|e| match e {
                kernel::StreamError::WouldBlock => Status::ErrTimedOut,
                kernel::StreamError::Closed => Status::ErrPeerClosed,
                kernel::StreamError::Failed => Status::ErrInvalidHandle,
            })
    }
    fn send(&self, channel: u64, bytes: &[u8], handles: &[u64]) -> Result<(), Status> {
        let resources = handles
            .iter()
            .map(|h| id(*h))
            .collect::<Result<Vec<_>, _>>()?;
        kernel::channel_write(
            id(channel)?,
            &kernel::Message {
                data: bytes.to_vec(),
                resources,
            },
        )
        .map_err(|_| Status::ErrPeerClosed)
    }
    fn close(&self, handle: u64) -> Result<(), Status> {
        kernel::resource_close(id(handle)?).map_err(|_| Status::ErrInvalidHandle)
    }
}
