#![no_std]

extern crate alloc;

use alloc::vec::Vec;
use bexos_tee_driver_client::*;
use core::ffi::c_void;
use core::panic::PanicInfo;

#[cfg(feature = "bexos_libc_runtime")]
#[global_allocator]
static GLOBAL_ALLOCATOR: bexos_libc::Allocator = bexos_libc::Allocator;

#[cfg(not(feature = "bexos_libc_runtime"))]
#[global_allocator]
static GLOBAL_ALLOCATOR: NoHeap = NoHeap;

#[cfg(not(feature = "bexos_libc_runtime"))]
struct NoHeap;

#[cfg(not(feature = "bexos_libc_runtime"))]
unsafe impl core::alloc::GlobalAlloc for NoHeap {
    unsafe fn alloc(&self, _layout: core::alloc::Layout) -> *mut u8 {
        core::ptr::null_mut()
    }
    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: core::alloc::Layout) {}
}

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    loop {}
}

#[derive(Clone)]
struct App {
    uuid: [u8; 16],
    version: u64,
    sessions: u32,
}

struct Context {
    apps: Vec<App>,
    pending: Option<TeeDriverCompletion>,
    next_session_id: u64,
    secure_os_version: u32,
    anti_rollback_version: u32,
    update: TeeUpdateState,
}

#[unsafe(no_mangle)]
pub extern "C" fn bexos_tee_driver_abi_version() -> u32 {
    ABI_VERSION
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn bexos_tee_driver_context_create(out: *mut *mut c_void) -> i32 {
    let Some(out) = (unsafe { out.as_mut() }) else {
        return STATUS_INVALID_ARGS;
    };
    let ctx = Context {
        apps: Vec::new(),
        pending: None,
        next_session_id: 1,
        secure_os_version: 1,
        anti_rollback_version: 0,
        update: TeeUpdateState {
            status: 1,
            phase: 1,
            active_slot: slot_name(b"A"),
            pending_slot: [0; 16],
            generation: 0,
            rollback_available: 0,
            reboot_required: 0,
        },
    };
    *out = alloc::boxed::Box::into_raw(alloc::boxed::Box::new(ctx)) as *mut c_void;
    STATUS_OK
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn bexos_tee_driver_context_destroy(ctx: *mut c_void) {
    if !ctx.is_null() {
        unsafe {
            drop(alloc::boxed::Box::from_raw(ctx as *mut Context));
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn bexos_tee_driver_submit(
    ctx: *mut c_void,
    request: *const TeeDriverRequest,
) -> i32 {
    let Some(ctx) = (unsafe { (ctx as *mut Context).as_mut() }) else {
        return STATUS_INVALID_ARGS;
    };
    let Some(request) = (unsafe { request.as_ref() }) else {
        return STATUS_INVALID_ARGS;
    };
    if ctx.pending.is_some() {
        return STATUS_RESOURCE_EXHAUSTED;
    }
    let mut completion = TeeDriverCompletion {
        request_id: request.request_id,
        status: STATUS_OK,
        ..Default::default()
    };
    match request.op {
        OP_PROBE => completion.info = info(ctx),
        OP_BUILTIN_DISCOVERY => completion.info = info(ctx),
        OP_LOAD_APP => {
            let uuid = request.app.uuid;
            if ctx.apps.iter().any(|app| app.uuid == uuid) {
                completion.status = STATUS_ALREADY_EXISTS;
            } else {
                ctx.apps.push(App {
                    uuid,
                    version: request.app.version,
                    sessions: 0,
                });
                completion.value = request.app.version;
            }
        }
        OP_UNLOAD_APP => match ctx.apps.iter().position(|app| app.uuid == request.app.uuid) {
            Some(index) if ctx.apps[index].sessions == 0 => {
                ctx.apps.swap_remove(index);
            }
            Some(_) => completion.status = STATUS_ACCESS_DENIED,
            None => completion.status = STATUS_NOT_FOUND,
        },
        OP_QUERY_APP => match ctx.apps.iter().find(|app| app.uuid == request.app.uuid) {
            Some(app) => completion.value = app.version,
            None => completion.status = STATUS_NOT_FOUND,
        },
        OP_CONNECT => match ctx
            .apps
            .iter_mut()
            .find(|app| app.uuid == request.endpoint.uuid)
        {
            Some(app) => {
                let session_id = ctx.next_session_id;
                ctx.next_session_id = ctx.next_session_id.saturating_add(1);
                app.sessions = app.sessions.saturating_add(1);
                completion.session_id = session_id;
            }
            None => completion.status = STATUS_NOT_FOUND,
        },
        OP_CLOSE => {
            if let Some(app) = ctx.apps.iter_mut().find(|app| app.sessions > 0) {
                app.sessions -= 1;
            } else {
                completion.status = STATUS_INVALID_ARGS;
            }
        }
        OP_INVOKE => {
            completion.status = copy_echo(request);
            if completion.status == STATUS_OK {
                completion.value = request.input.len.saturating_add(22) as u64;
            }
        }
        OP_TRANSPORT_RESET => {}
        OP_CORE_STAGE | OP_CORE_ACTIVATE => {
            if request.core.generation <= u64::from(ctx.anti_rollback_version) {
                completion.status = STATUS_VERIFY_FAILED;
                ctx.update.status = 5;
                ctx.update.phase = 8;
            } else {
                ctx.anti_rollback_version = request.core.generation as u32;
                ctx.secure_os_version = ctx.secure_os_version.saturating_add(1);
                ctx.update.status = 4;
                ctx.update.phase = if request.core.activation == 2 { 5 } else { 7 };
                ctx.update.generation = request.core.generation;
                ctx.update.rollback_available = 1;
                ctx.update.reboot_required = u32::from(request.core.activation == 2);
                ctx.update.pending_slot = if request.core.activation == 2 {
                    slot_name(b"B")
                } else {
                    [0; 16]
                };
                completion.value = u64::from(ctx.secure_os_version);
            }
        }
        OP_CORE_STATUS => completion.update = ctx.update,
        _ => completion.status = STATUS_INVALID_ARGS,
    }
    if completion.update == TeeUpdateState::default() {
        completion.update = ctx.update;
    }
    ctx.pending = Some(completion);
    STATUS_OK
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn bexos_tee_driver_poll(
    ctx: *mut c_void,
    completion: *mut TeeDriverCompletion,
) -> i32 {
    let Some(ctx) = (unsafe { (ctx as *mut Context).as_mut() }) else {
        return STATUS_INVALID_ARGS;
    };
    let Some(completion) = (unsafe { completion.as_mut() }) else {
        return STATUS_INVALID_ARGS;
    };
    match ctx.pending.take() {
        Some(pending) => {
            *completion = pending;
            STATUS_OK
        }
        None => STATUS_TIMED_OUT,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn bexos_tee_driver_cancel(ctx: *mut c_void, request_id: u64) -> i32 {
    let Some(ctx) = (unsafe { (ctx as *mut Context).as_mut() }) else {
        return STATUS_INVALID_ARGS;
    };
    if ctx
        .pending
        .is_some_and(|completion| completion.request_id == request_id)
    {
        ctx.pending = None;
        STATUS_OK
    } else {
        STATUS_NOT_FOUND
    }
}

fn info(ctx: &Context) -> TeeDriverInfo {
    TeeDriverInfo {
        present: 1,
        kind: TEE_KIND_SOFTWARE,
        secure_os_version: ctx.secure_os_version,
        anti_rollback_version: ctx.anti_rollback_version,
        abi_version: ABI_VERSION,
        flags: 0,
    }
}

fn copy_echo(request: &TeeDriverRequest) -> i32 {
    if request.output.ptr.is_null() {
        return STATUS_INVALID_ARGS;
    }
    let needed = request.input.len.saturating_add(22);
    if request.output.len < needed {
        return STATUS_BUFFER_TOO_SMALL;
    }
    let output = unsafe { core::slice::from_raw_parts_mut(request.output.ptr, request.output.len) };
    let input = unsafe { core::slice::from_raw_parts(request.input.ptr, request.input.len) };
    let mut cursor = 0;
    cursor = put(output, cursor, b"BEXTEE");
    cursor = put(output, cursor, &request.session_id.to_le_bytes());
    cursor = put(output, cursor, &request.command_id.to_le_bytes());
    cursor = put(output, cursor, &(request.input.len as u32).to_le_bytes());
    let _ = put(output, cursor, input);
    STATUS_OK
}

fn put(out: &mut [u8], cursor: usize, bytes: &[u8]) -> usize {
    out[cursor..cursor + bytes.len()].copy_from_slice(bytes);
    cursor + bytes.len()
}

fn slot_name(bytes: &[u8]) -> [u8; 16] {
    let mut slot = [0; 16];
    slot[..bytes.len().min(16)].copy_from_slice(&bytes[..bytes.len().min(16)]);
    slot
}

// This freestanding library aborts on panic and never unwinds.
#[unsafe(no_mangle)]
pub extern "C" fn rust_eh_personality() {}
