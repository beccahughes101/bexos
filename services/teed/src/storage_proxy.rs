//! teed owns the normal-world storage proxy; the driver only pumps raw TIPC.
use bexos_tee_driver_client::*;
use bexos_userspace::Channel;
use core::ffi::c_void;

pub struct StorageProxy {
    pub channel: Channel,
}
impl StorageProxy {
    pub fn handler(&mut self) -> StorageProxyHandler {
        StorageProxyHandler {
            context: (self as *mut Self).cast(),
            dispatch,
        }
    }
}
unsafe extern "C" fn dispatch(
    context: *mut c_void,
    request: *const u8,
    request_len: usize,
    response: *mut u8,
    capacity: usize,
    written: *mut usize,
) -> i32 {
    if context.is_null()
        || request.is_null()
        || response.is_null()
        || written.is_null()
        || request_len > 8192
    {
        return STATUS_INVALID_ARGS;
    }
    let proxy = unsafe { &mut *context.cast::<StorageProxy>() };
    let request = unsafe { core::slice::from_raw_parts(request, request_len) };
    match bexos_rpmb_proxy::dispatch(request, |frames, count| {
        bexos_rpmb_proxy::exchange(proxy.channel, frames, count)
    }) {
        Ok(bytes) if bytes.len() <= capacity => {
            unsafe {
                core::ptr::copy_nonoverlapping(bytes.as_ptr(), response, bytes.len());
                *written = bytes.len();
            }
            STATUS_OK
        }
        Ok(_) => STATUS_BUFFER_TOO_SMALL,
        Err(status) => status,
    }
}
