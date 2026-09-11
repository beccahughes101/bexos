//! Allocation-free reception for bounded graphics/input streams without handles.
use bexos_userspace::{Channel, Memory};
use kernel_fidl::{FidlDecode, FidlEncode, HandleRef, Status};

/// Read one message into caller-owned storage. Capacity failures leave it queued;
/// callers must reject an oversized stream rather than retry it indefinitely.
/// Storage includes the kernel response envelope in addition to the payload.
pub fn read_no_handles<const N: usize>(
    channel: Channel,
    storage: &mut [u8; N],
) -> Result<&[u8], Status> {
    const {
        assert!(N >= ENVELOPE_BYTES && N <= bexos_userspace::ipc::MAX_MESSAGE);
    }
    let mut request = [0; 32];
    let encoded = kernel_fidl::ChannelControlReadMessageRequest {
        channel: HandleRef { raw: channel.0 },
        max_bytes: (N - ENVELOPE_BYTES) as u32,
        max_handles: 32,
    }
    .encode(&mut request, &mut [HandleRef { raw: 0 }; 1])
    .map_err(|_| Status::ErrInvalidArgs)?;
    let ordinal = kernel_fidl::CHANNEL_CONTROL_PUBLIC_METHODS
        .iter()
        .find(|method| method.name == "ReadMessage")
        .ok_or(Status::ErrInvalidArgs)?
        .ordinal;
    let mut handles = [0; 32];
    let (bytes, count) = bexos_userspace::syscall::fidl(
        1,
        ordinal,
        &request[..encoded.bytes],
        &[channel.0],
        storage,
        &mut handles,
    )
    .map_err(bexos_userspace::ipc::status)?;
    if count != 0 {
        for handle in &handles[..count] {
            let _ = Memory::close(*handle);
        }
        return Err(Status::ErrInvalidArgs);
    }
    let response = kernel_fidl::ChannelControlReadMessageResponse::decode(&storage[..bytes], &[])
        .map_err(|_| Status::ErrInvalidArgs)?;
    bexos_userspace::ipc::check(response.status)?;
    Ok(response.data)
}

/// Reserved space for the kernel ReadMessage response and handle metadata.
pub const ENVELOPE_BYTES: usize = 512;
