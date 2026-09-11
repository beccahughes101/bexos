//! FIDL transport over the guest's checked resource table.
use crate::bexos::wasm::kernel;
pub use kernel::Message;
pub fn clear_sensitive(bytes: &mut [u8]) {
    for byte in bytes {
        // Password transport buffers must be cleared even just before drop.
        unsafe { core::ptr::write_volatile(byte, 0) };
    }
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
}
pub fn exchange(
    channel: u32,
    ordinal: u64,
    body: &[u8],
    resources: Vec<u32>,
) -> Result<Message, String> {
    exchange_timeout(channel, ordinal, body, resources, 30_000_000_000)
}
pub fn exchange_timeout(
    channel: u32,
    ordinal: u64,
    body: &[u8],
    resources: Vec<u32>,
    timeout_ns: u64,
) -> Result<Message, String> {
    let mut data = ordinal.to_le_bytes().to_vec();
    data.extend(body);
    let mut message = Message { data, resources };
    let result = kernel::channel_write(channel, &message);
    clear_sensitive(&mut message.data);
    result.map_err(|_| "IPC send failed")?;
    let start = kernel::monotonic_ns();
    loop {
        match kernel::channel_read_checked(channel, 32768, 16) {
            Ok(m) => return Ok(m),
            Err(kernel::StreamError::WouldBlock)
                if kernel::monotonic_ns().saturating_sub(start) < timeout_ns =>
            {
                // Let the provider and compositor run without burning the
                // dispatch fuel budget while durable operations are pending.
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            _ => return Err("IPC response unavailable".into()),
        }
    }
}
pub fn find(name: &str) -> Result<u32, String> {
    kernel::resource_find(name).ok_or_else(|| format!("Missing {name} grant"))
}
pub fn close(id: u32) {
    let _ = kernel::resource_close(id);
}
pub fn clone_resource(id: u32) -> Result<u32, String> {
    kernel::resource_clone(id).map_err(|_| "Resource duplication failed".into())
}
