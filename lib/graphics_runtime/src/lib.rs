extern crate alloc;
pub mod discovery;
pub mod mapping;
pub mod migration;
pub mod response;
pub mod rpc;
use bexos_userspace::{Channel, Memory};
use graphics_fidl::*;
pub use mapping::Mapping;
pub fn now_us() -> u64 {
    ((bexos_userspace::syscall::ticks() as u128 * 1_000_000)
        / bexos_userspace::syscall::frequency() as u128) as u64
}
pub fn surface(s: Surface) -> Result<bexos_graphics::Surface, bexos_graphics::Error> {
    Ok(bexos_graphics::Surface {
        width: s.width,
        height: s.height,
        stride: s.stride,
        format: s.format.try_into()?,
    })
}
pub fn wire(s: bexos_graphics::Surface) -> Surface {
    Surface {
        width: s.width,
        height: s.height,
        stride: s.stride,
        format: s.format as u32,
    }
}
pub fn refs(handles: &[u64]) -> Vec<HandleRef> {
    handles.iter().map(|h| HandleRef { raw: *h }).collect()
}
pub fn close(handles: &[u64]) {
    for h in handles {
        let _ = Memory::close(*h);
    }
}
pub fn reply(channel: Channel, value: &impl FidlEncode) {
    let _ = try_reply(channel, value);
}
pub fn try_reply(channel: Channel, value: &impl FidlEncode) -> bool {
    let mut out = [0; 1024];
    try_reply_in(channel, value, &mut out)
}
pub fn try_reply_in(channel: Channel, value: &impl FidlEncode, out: &mut [u8]) -> bool {
    let mut hs = [HandleRef { raw: 0 }; 4];
    if let Ok(n) = value.encode(out, &mut hs) {
        let mut handles = [0; 4];
        for (out, h) in handles.iter_mut().zip(hs).take(n.handles) {
            *out = h.raw;
        }
        if channel
            .send(&out[..n.bytes], &handles[..n.handles])
            .is_err()
        {
            close(&handles[..n.handles]);
            return false;
        }
        return true;
    }
    false
}
pub fn call<Q: FidlEncode, R: for<'a> FidlDecode<'a> + FidlEncode>(
    channel: &mut Channel,
    ordinal: u64,
    value: &Q,
) -> Result<R, Status> {
    call_impl(channel, ordinal, value, None)
}
/// Consumes exactly these request handles, including encode/admission failure.
/// Use for clients transferring duplicates while retaining their original VMO
/// or channel. Responses transfer ownership to the decoded resource response.
pub fn call_owned<Q: FidlEncode, R: for<'a> FidlDecode<'a> + FidlEncode>(
    channel: &mut Channel,
    ordinal: u64,
    value: &Q,
    owned: &[u64],
) -> Result<R, Status> {
    call_impl(channel, ordinal, value, Some(owned))
}
fn call_impl<Q: FidlEncode, R: for<'a> FidlDecode<'a> + FidlEncode>(
    channel: &mut Channel,
    ordinal: u64,
    value: &Q,
    owned: Option<&[u64]>,
) -> Result<R, Status> {
    if channel.0 == 0 {
        if let Some(handles) = owned {
            close(handles);
        }
        return Err(Status::ErrIo);
    }
    let mut out = vec![0; 4096];
    let mut hs = [HandleRef { raw: 0 }; 4];
    let n = match value.encode(&mut out[8..], &mut hs) {
        Ok(n) => n,
        Err(_) => {
            if let Some(handles) = owned {
                close(handles);
            }
            return Err(Status::ErrInvalidArgs);
        }
    };
    out[..8].copy_from_slice(&ordinal.to_le_bytes());
    let handles: Vec<_> = hs[..n.handles].iter().map(|h| h.raw).collect();
    if owned.is_some_and(|expected| expected != handles) {
        close(owned.unwrap());
        return Err(Status::ErrInvalidArgs);
    }
    if channel.send(&out[..n.bytes + 8], &handles).is_err() {
        close(&handles);
        return Err(Status::ErrIo);
    }
    let start = now_us();
    loop {
        match channel.try_recv() {
            Ok(m) => {
                if let Ok(response) =
                    response::decode_owned(&m.bytes, &refs(&m.handles), &mut out, &mut hs)
                {
                    return Ok(response);
                }
                close(&m.handles);
                return Err(Status::ErrInvalidArgs);
            }
            Err(kernel_fidl::Status::ErrTimedOut) => {}
            Err(_) => return Err(Status::ErrIo),
        }
        if now_us().saturating_sub(start) > 60_000_000 {
            let _ = Memory::close(channel.0);
            channel.0 = 0;
            return Err(Status::ErrTimedOut);
        }
        wait(&[*channel], start.saturating_add(60_000_001));
    }
}
pub fn envelope(bytes: &[u8]) -> Option<(u64, &[u8])> {
    Some((
        u64::from_le_bytes(bytes.get(..8)?.try_into().ok()?),
        &bytes[8..],
    ))
}

pub mod canvas;
pub mod presentation;
/// Sleep until a timer deadline or an IPC/migration event; keep IPC responsive between frames.
pub fn wait(channels: &[Channel], deadline_us: u64) {
    let mut items = [kernel_fidl::InlineVectorStruct1 {
        h: kernel_fidl::HandleRef { raw: 0 },
        signals: kernel_fidl::Signals(0),
    }; 64];
    let count = channels.len().min(items.len());
    for (item, c) in items.iter_mut().zip(channels) {
        *item = kernel_fidl::InlineVectorStruct1 {
            h: kernel_fidl::HandleRef { raw: c.0 },
            signals: kernel_fidl::Signals(
                kernel_fidl::Signals::READABLE.0 | kernel_fidl::Signals::PEER_CLOSED.0,
            ),
        };
    }
    let _: Result<kernel_fidl::TaskControlWaitManyResponse, _> =
        bexos_userspace::ipc::kernel_call_buffered(
            3,
            "WaitMany",
            kernel_fidl::TASK_CONTROL_PUBLIC_METHODS,
            &kernel_fidl::TaskControlWaitManyRequest {
                items: kernel_fidl::WireVector::from_slice(&items[..count]),
                deadline_nanos: deadline_us.saturating_mul(1000).min(i64::MAX as u64) as i64,
            },
            &mut [0; 2048],
            &mut [0; 32],
        );
}

pub mod scheduling;
pub mod stream;

pub mod scanout;

pub mod flatland;
