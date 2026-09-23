#![no_std]

extern crate alloc;

mod firmware;
#[cfg(target_arch = "x86_64")]
mod ql_monitor;
mod ql_proto;
mod ql_tipc;

use alloc::{format, vec::Vec};
use bexos_secure_monitor_abi::{ACTIVATE_LIVE_NOW, ACTIVATE_ON_REBOOT, Status as MonitorStatus};
use bexos_tee_driver_client::*;
use bexos_userspace::syscall;
use core::ffi::c_void;
use core::panic::PanicInfo;
use kernel_fidl::{
    SECURE_MONITOR_PUBLIC_METHODS, SecureMonitorCallRequest, SecureMonitorCallResponse,
};
use ql_tipc::{QueuedTipc, TipcSession, endpoint_port};

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

#[unsafe(no_mangle)]
pub extern "C" fn rust_eh_personality() {}

const TRUSTY_API_VERSION: u64 = 0xbc00_000b;
const TRUSTY_API_VERSION_CURRENT: u64 = 2;
const KEYMINT_UUID: [u8; 16] = [
    0x5f, 0x90, 0x2a, 0xce, 0x5e, 0x5c, 0x4c, 0xd8, 0xae, 0x54, 0x87, 0xb8, 0x8c, 0x22, 0xdd, 0xaf,
];
const GATEKEEPER_UUID: [u8; 16] = [
    0x38, 0xba, 0x0c, 0xdc, 0xdf, 0x0e, 0x11, 0xe4, 0x98, 0x69, 0x23, 0x3f, 0xb6, 0xae, 0x47, 0x95,
];
const AVB_UUID: [u8; 16] = [
    0x90, 0x5b, 0xcb, 0x84, 0x2d, 0xc4, 0x41, 0x9d, 0xb9, 0x51, 0x72, 0x55, 0x02, 0x7d, 0x31, 0x8c,
];
const AUTHMGR_BE_UUID: [u8; 16] = [
    0xf4, 0x76, 0x89, 0x56, 0x62, 0xd9, 0x49, 0x04, 0x95, 0x12, 0x86, 0xdf, 0x36, 0x0d, 0x8d, 0x50,
];
const STORAGE_UUID: [u8; 16] = [
    0xce, 0xa8, 0x70, 0x6d, 0x6c, 0xb4, 0x49, 0xf3, 0xb9, 0x94, 0x29, 0xe0, 0xe4, 0x78, 0xbd, 0x29,
];
const ORCHESTRATOR_UUID: [u8; 16] = [
    0x2b, 0x45, 0x58, 0x4f, 0x53, 0x06, 0x40, 0x02, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x06,
];

struct Context {
    pending: Option<TeeDriverCompletion>,
    storage_handler: Option<StorageProxyHandler>,
    transport: Option<QueuedTipc>,
    sessions: Vec<TipcSession>,
    next_session_id: u64,
    probed: bool,
    info: TeeDriverInfo,
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
        pending: None,
        storage_handler: None,
        transport: None,
        sessions: Vec::new(),
        next_session_id: 1,
        probed: false,
        info: TeeDriverInfo {
            present: 0,
            kind: TEE_KIND_TRUSTY,
            secure_os_version: 0,
            anti_rollback_version: 0,
            abi_version: ABI_VERSION,
            flags: 0,
        },
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
        info: ctx.info,
        update: ctx.update,
        ..Default::default()
    };
    match request.op {
        OP_PROBE => match trusty_probe(ctx) {
            Ok(info) => {
                ctx.probed = true;
                ctx.info = info;
                completion.info = info;
            }
            Err(status) => completion.status = status,
        },
        OP_TRANSPORT_RESET => completion.status = trusty_reset(ctx),
        OP_BUILTIN_DISCOVERY => {
            if !ctx.probed {
                completion.status = STATUS_UNAVAILABLE;
            }
            completion.info = ctx.info;
        }
        OP_QUERY_APP => {
            completion.status = if ctx.probed {
                STATUS_OK
            } else {
                STATUS_ACCESS_DENIED
            };
        }
        OP_STORAGE_PROXY_HANDLER => {
            if request.input.len != core::mem::size_of::<StorageProxyHandler>()
                || request.input.ptr.is_null()
            {
                completion.status = STATUS_INVALID_ARGS;
            } else {
                ctx.storage_handler = Some(unsafe {
                    core::ptr::read_unaligned(request.input.ptr.cast::<StorageProxyHandler>())
                });
                if let Some(transport) = ctx.transport.as_mut() {
                    transport.storage_handler = ctx.storage_handler;
                }
            }
        }
        OP_CONNECT => {
            completion.status = trusty_connect(ctx, request, &mut completion);
        }
        OP_CLOSE => {
            completion.status = trusty_close(ctx, request.session_id);
        }
        OP_INVOKE => {
            completion.status = trusty_invoke(ctx, request, &mut completion);
        }
        OP_RECV => {
            completion.status = trusty_recv(ctx, request, &mut completion);
        }
        OP_SEND => {
            completion.status = trusty_send(ctx, request);
        }
        OP_CORE_STAGE | OP_CORE_ACTIVATE => {
            completion.status = stage_or_activate_core(ctx, request);
            completion.update = ctx.update;
            completion.value = u64::from(ctx.info.secure_os_version);
        }
        OP_CORE_STATUS => {
            match firmware::status() {
                Ok(report) => apply_firmware_report(ctx, report),
                Err(status) => completion.status = status,
            }
            completion.update = ctx.update;
        }
        // This image exposes only its authenticated built-in catalogue. The
        // package-manager ABI is not a signed Trusty application loader.
        OP_LOAD_APP | OP_UNLOAD_APP => completion.status = STATUS_ACCESS_DENIED,
        _ => completion.status = STATUS_INVALID_ARGS,
    }
    ctx.pending = Some(completion);
    STATUS_OK
}

fn trusty_connect(
    ctx: &mut Context,
    request: &TeeDriverRequest,
    completion: &mut TeeDriverCompletion,
) -> i32 {
    if !ctx.probed {
        return STATUS_ACCESS_DENIED;
    }
    if !is_builtin_uuid(request.endpoint.uuid) {
        return STATUS_NOT_FOUND;
    }
    let Some(port) = endpoint_port(request.endpoint.uuid, &request.endpoint) else {
        return STATUS_INVALID_ARGS;
    };
    let id = if request.session_id == 0 {
        let id = ctx.next_session_id;
        ctx.next_session_id = ctx.next_session_id.saturating_add(1).max(1);
        id
    } else {
        request.session_id
    };
    let Some(transport) = ctx.transport.as_mut() else {
        return STATUS_UNAVAILABLE;
    };
    let session = match transport.connect(id, request.endpoint.uuid, &port) {
        Ok(session) => session,
        Err(status) => {
            log_status("QL-TIPC connect", status);
            return status;
        }
    };
    ctx.sessions.push(session);
    completion.session_id = id;
    STATUS_OK
}

fn trusty_close(ctx: &mut Context, session_id: u64) -> i32 {
    match ctx
        .sessions
        .iter()
        .position(|session| session.id == session_id)
    {
        Some(index) => {
            let session = ctx.sessions.swap_remove(index);
            match ctx.transport.as_mut() {
                Some(transport) => transport
                    .close(session.handle)
                    .map_or_else(|status| status, |_| STATUS_OK),
                None => STATUS_UNAVAILABLE,
            }
        }
        None => STATUS_NOT_FOUND,
    }
}

fn trusty_invoke(
    ctx: &mut Context,
    request: &TeeDriverRequest,
    completion: &mut TeeDriverCompletion,
) -> i32 {
    let Some(session) = ctx
        .sessions
        .iter()
        .find(|session| session.id == request.session_id)
    else {
        return STATUS_NOT_FOUND;
    };
    if request.output.ptr.is_null() {
        return STATUS_INVALID_ARGS;
    }
    let output = unsafe { core::slice::from_raw_parts_mut(request.output.ptr, request.output.len) };
    let input = unsafe { core::slice::from_raw_parts(request.input.ptr, request.input.len) };
    let Some(transport) = ctx.transport.as_mut() else {
        return STATUS_UNAVAILABLE;
    };
    let handle = session.handle;
    let result = if session.uuid == KEYMINT_UUID {
        transport.invoke_keymint(handle, input, output)
    } else {
        transport.invoke(handle, input, output)
    };
    match result {
        Ok(written) => {
            completion.value = written as u64;
            STATUS_OK
        }
        Err(status) => status,
    }
}

fn trusty_recv(
    ctx: &mut Context,
    request: &TeeDriverRequest,
    completion: &mut TeeDriverCompletion,
) -> i32 {
    let Some(session) = ctx
        .sessions
        .iter()
        .find(|session| session.id == request.session_id)
    else {
        return STATUS_NOT_FOUND;
    };
    if request.output.ptr.is_null() || request.output.len == 0 {
        return STATUS_INVALID_ARGS;
    }
    let output = unsafe { core::slice::from_raw_parts_mut(request.output.ptr, request.output.len) };
    let Some(transport) = ctx.transport.as_mut() else {
        return STATUS_UNAVAILABLE;
    };
    match transport.try_recv(session.handle, output) {
        Ok(written) => {
            completion.value = written as u64;
            STATUS_OK
        }
        Err(status) => status,
    }
}

fn trusty_send(ctx: &mut Context, request: &TeeDriverRequest) -> i32 {
    let Some(session) = ctx
        .sessions
        .iter()
        .find(|session| session.id == request.session_id)
    else {
        return STATUS_NOT_FOUND;
    };
    if request.input.ptr.is_null() && request.input.len != 0 {
        return STATUS_INVALID_ARGS;
    }
    let input = unsafe { core::slice::from_raw_parts(request.input.ptr, request.input.len) };
    let Some(transport) = ctx.transport.as_mut() else {
        return STATUS_UNAVAILABLE;
    };
    transport
        .send(session.handle, input)
        .map_or_else(|status| status, |_| STATUS_OK)
}

fn trusty_reset(ctx: &mut Context) -> i32 {
    if !ctx.probed {
        return STATUS_UNAVAILABLE;
    }
    ctx.sessions.clear();
    match ctx.transport.as_mut() {
        Some(transport) => transport
            .reset()
            .map_or_else(|status| status, |_| STATUS_OK),
        None => match QueuedTipc::create() {
            Ok(transport) => {
                let mut transport = transport;
                transport.storage_handler = ctx.storage_handler;
                ctx.transport = Some(transport);
                STATUS_OK
            }
            Err(status) => status,
        },
    }
}

fn rebind_live_sessions(ctx: &mut Context, fence_storage_writes: bool) -> Result<(), i32> {
    let mut retired = ctx.transport.take();
    #[cfg(target_arch = "aarch64")]
    if let Some(transport) = retired.as_mut() {
        transport.retire_after_owner_cutover();
    }
    let mut transport = QueuedTipc::create().inspect_err(|status| {
        log_status("candidate QL-TIPC create", *status);
    })?;
    transport.storage_handler = ctx.storage_handler;
    transport.fence_storage_writes(fence_storage_writes);
    let mut sessions = ctx.sessions.clone();
    sessions.sort_by_key(|session| u8::from(session.uuid != STORAGE_UUID));
    let mut rebound = Vec::with_capacity(sessions.len());
    for session in sessions {
        rebound.push(
            transport
                .connect(session.id, session.uuid, &session.port)
                .inspect_err(|status| log_status("retained session rebind", *status))?,
        );
    }
    ctx.sessions = rebound;
    ctx.transport = Some(transport);
    drop(retired);
    Ok(())
}

#[cfg(target_arch = "aarch64")]
fn probe_candidate_services(
    ctx: &mut Context,
    generation: u64,
    cutover_started_ms: u64,
    activation: Option<&firmware::Activation>,
) -> Result<(), i32> {
    let within_deadline =
        || bexos_userspace::live_migration::now_ms().saturating_sub(cutover_started_ms) < 150;
    if !within_deadline() {
        return Err(STATUS_TIMED_OUT);
    }
    let measured = match activation {
        Some(activation) => {
            activation.active_generation(bexos_secure_monitor_abi::firmware::TRUSTY)
        }
        None => firmware::active_generation(bexos_secure_monitor_abi::firmware::TRUSTY),
    };
    if u64::from(measured.inspect_err(|status| log_status("candidate generation query", *status))?)
        != generation
    {
        syscall::log("tee-driver-trusty: candidate generation measurement mismatch\n");
        return Err(STATUS_VERIFY_FAILED);
    }
    let transport = ctx.transport.as_mut().ok_or(STATUS_UNAVAILABLE)?;
    let mut opened = Vec::new();
    for (index, uuid) in [
        STORAGE_UUID,
        KEYMINT_UUID,
        GATEKEEPER_UUID,
        AVB_UUID,
        AUTHMGR_BE_UUID,
        ORCHESTRATOR_UUID,
    ]
    .into_iter()
    .enumerate()
    {
        if ctx.sessions.iter().any(|session| session.uuid == uuid) {
            continue;
        }
        let port = endpoint_port(
            uuid,
            &TeeEndpoint {
                uuid,
                ..Default::default()
            },
        )
        .ok_or(STATUS_INVALID_ARGS)?;
        opened.push(
            transport
                .connect(u64::MAX - index as u64, uuid, &port)
                .inspect_err(|status| log_status("candidate required-service connect", *status))?,
        );
        if !within_deadline() {
            return Err(STATUS_TIMED_OUT);
        }
    }
    let orchestrator = if let Some(session) = ctx
        .sessions
        .iter()
        .chain(opened.iter())
        .find(|session| session.uuid == ORCHESTRATOR_UUID)
    {
        session.handle
    } else {
        return Err(STATUS_NOT_FOUND);
    };
    let mut request = [0u8; 16];
    request[..4].copy_from_slice(&1u32.to_le_bytes());
    request[4..8].copy_from_slice(&0x203u32.to_le_bytes());
    let mut response = [0u8; 16];
    let count = transport
        .invoke(orchestrator, &request, &mut response)
        .inspect_err(|status| log_status("candidate measurement probe", *status))?;
    let reported = if count == response.len()
        && u32::from_le_bytes(response[..4].try_into().unwrap()) == 1
        && u32::from_le_bytes(response[4..8].try_into().unwrap()) == 0
    {
        u64::from_le_bytes(response[8..16].try_into().unwrap())
    } else {
        return Err(STATUS_VERIFY_FAILED);
    };
    for session in opened.into_iter().rev() {
        transport
            .close(session.handle)
            .inspect_err(|status| log_status("candidate service-probe close", *status))?;
    }
    (reported == generation && within_deadline())
        .then_some(())
        .ok_or(STATUS_VERIFY_FAILED)
}

fn is_builtin_uuid(uuid: [u8; 16]) -> bool {
    uuid == KEYMINT_UUID
        || uuid == GATEKEEPER_UUID
        || uuid == AVB_UUID
        || uuid == AUTHMGR_BE_UUID
        || uuid == STORAGE_UUID
        || uuid == ORCHESTRATOR_UUID
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

fn trusty_probe(ctx: &mut Context) -> Result<TeeDriverInfo, i32> {
    #[cfg(not(target_arch = "x86_64"))]
    let regs = secure_monitor(
        TRUSTY_API_VERSION,
        TRUSTY_API_VERSION_CURRENT,
        0,
        0,
        0,
        0,
        0,
        0,
    )
    .inspect_err(|status| log_status("API version SMC", *status))?;
    #[cfg(not(target_arch = "x86_64"))]
    if regs[0] != TRUSTY_API_VERSION_CURRENT {
        return Err(STATUS_UNAVAILABLE);
    }
    // The negotiated transport ABI is not a Trusty firmware generation. Ask
    // the permanent execution owner on both maintained architectures.
    let version = firmware::active_trusty_generation().unwrap_or(0);
    #[cfg(target_arch = "aarch64")]
    let boot_report = firmware::status().inspect_err(|status| {
        log_status("firmware status", *status);
    })?;
    if ctx.transport.is_none() {
        ctx.transport =
            Some(QueuedTipc::create().inspect_err(|status| log_status("QL-TIPC create", *status))?);
    }
    #[cfg(target_arch = "aarch64")]
    {
        if boot_report.outcome == bexos_secure_monitor_abi::firmware::APPLYING
            && boot_report.component == bexos_secure_monitor_abi::firmware::TRUSTY
        {
            ctx.transport
                .as_mut()
                .ok_or(STATUS_UNAVAILABLE)?
                .fence_storage_writes(true);
            let readiness_started = bexos_userspace::live_migration::now_ms();
            probe_candidate_services(ctx, boot_report.generation, readiness_started, None)?;
            let committed = firmware::resolve(true)?;
            if committed.outcome != bexos_secure_monitor_abi::firmware::COMMITTED
                || committed.generation != boot_report.generation
            {
                return Err(STATUS_UNAVAILABLE);
            }
            ctx.transport
                .as_mut()
                .ok_or(STATUS_UNAVAILABLE)?
                .fence_storage_writes(false);
            syscall::log(
                "tee-driver-trusty: reboot trial services accepted and authenticated selection committed\n",
            );
        }
    }
    Ok(TeeDriverInfo {
        present: 1,
        kind: TEE_KIND_TRUSTY,
        secure_os_version: version.max(ctx.info.secure_os_version),
        anti_rollback_version: version.max(ctx.info.anti_rollback_version),
        abi_version: ABI_VERSION,
        flags: 1,
    })
}

pub(crate) fn log_status(operation: &str, status: i32) {
    syscall::log(&format!(
        "tee-driver-trusty: {operation} failed status={status}\n"
    ));
}

fn stage_or_activate_core(ctx: &mut Context, request: &TeeDriverRequest) -> i32 {
    if !ctx.probed {
        return STATUS_ACCESS_DENIED;
    }
    if request.op != OP_CORE_ACTIVATE
        || request.core.image.len == 0
        || request.core.image.ptr.is_null()
        || request.core.image.physical == 0
        || request.core.target_len == 0
        || request.core.target_ptr.is_null()
        || request.core.target_len > 64
        || !matches!(request.core.activation, 1 | 2)
    {
        return STATUS_INVALID_ARGS;
    }
    let target =
        unsafe { core::slice::from_raw_parts(request.core.target_ptr, request.core.target_len) };
    if !matches!(
        target,
        b"qemu-aarch64-tee" | b"qemu-x86_64-tee" | b"qemu-x86_64-monitor"
    ) {
        set_update_failed(ctx, request.core.generation, 9);
        return STATUS_INVALID_ARGS;
    }
    if (target == b"qemu-aarch64-tee") != cfg!(target_arch = "aarch64") {
        set_update_failed(ctx, request.core.generation, 9);
        return STATUS_INVALID_ARGS;
    }
    // Component floors are independent. A newer Trusty must not prevent a
    // valid monitor update whose generation exceeds the monitor's own floor.
    let component = if target == b"qemu-x86_64-monitor" {
        bexos_secure_monitor_abi::firmware::HYPERVISOR
    } else {
        bexos_secure_monitor_abi::firmware::TRUSTY
    };
    let active = match firmware::active_generation(component) {
        Ok(active) if active != 0 => active,
        _ => {
            set_update_failed(ctx, request.core.generation, 9);
            return STATUS_UNAVAILABLE;
        }
    };
    if request.core.generation <= active {
        set_update_failed(ctx, request.core.generation, 8);
        return STATUS_VERIFY_FAILED;
    }
    if request.core.image.len as u64 > bexos_secure_monitor_abi::firmware::MAX_BUNDLE_BYTES
        || request.core.generation > u64::from(u32::MAX)
    {
        return STATUS_INVALID_ARGS;
    }
    let image =
        unsafe { core::slice::from_raw_parts(request.core.image.ptr, request.core.image.len) };
    if <[u8; 32]>::from(blake3::hash(image)) != request.core.hash {
        set_update_failed(ctx, request.core.generation, 8);
        return STATUS_VERIFY_FAILED;
    }
    match activate_registered_core(ctx, request, image) {
        Ok(report) => {
            apply_firmware_report(ctx, report);
            STATUS_OK
        }
        Err(status) => {
            set_update_failed(ctx, request.core.generation, 9);
            if let Ok(report) = firmware::status() {
                if report.generation == request.core.generation && report.component == component {
                    apply_firmware_report(ctx, report);
                }
            }
            status
        }
    }
}

fn apply_firmware_report(ctx: &mut Context, report: firmware::Report) {
    use bexos_secure_monitor_abi::firmware as abi;
    let slot = match report.slot {
        1 => slot_name(b"A"),
        2 => slot_name(b"B"),
        _ => [0; 16],
    };
    ctx.update.generation = report.generation;
    // A staged/rejected upload has no selected slot. Do not retain the slot
    // reported by an earlier transaction for the other firmware component.
    ctx.update.active_slot = [0; 16];
    ctx.update.pending_slot = [0; 16];
    ctx.update.reboot_required = 0;
    ctx.update.rollback_available = 0;
    (ctx.update.status, ctx.update.phase) = match report.outcome {
        abi::IDLE => (1, 1),
        abi::STAGED => (2, 3),
        abi::APPLYING => (3, 4),
        abi::PENDING => {
            ctx.update.active_slot = if report.slot == 1 {
                slot_name(b"B")
            } else {
                slot_name(b"A")
            };
            ctx.update.pending_slot = slot;
            ctx.update.reboot_required = 1;
            (2, 5)
        }
        abi::COMMITTED => {
            ctx.update.active_slot = slot;
            if report.component == abi::TRUSTY {
                ctx.info.secure_os_version = report.generation as u32;
                ctx.info.anti_rollback_version = report.generation as u32;
            }
            (4, 7)
        }
        abi::ROLLED_BACK => {
            ctx.update.active_slot = if report.slot == 1 {
                slot_name(b"B")
            } else {
                slot_name(b"A")
            };
            (5, 8)
        }
        abi::RECOVERY_REQUIRED => (5, 10),
        _ => (5, 9),
    };
}

fn activate_registered_core(
    ctx: &mut Context,
    request: &TeeDriverRequest,
    image: &[u8],
) -> Result<firmware::Report, i32> {
    ctx.update.status = 3;
    ctx.update.phase = 2;
    ctx.update.pending_slot = if ctx.update.active_slot == slot_name(b"A") {
        slot_name(b"B")
    } else {
        slot_name(b"A")
    };
    ctx.update.generation = request.core.generation;
    ctx.update.phase = 4;
    let activation = match request.core.activation {
        1 => ACTIVATE_LIVE_NOW,
        2 => ACTIVATE_ON_REBOOT,
        _ => return Err(STATUS_INVALID_ARGS),
    };
    let target =
        unsafe { core::slice::from_raw_parts(request.core.target_ptr, request.core.target_len) };
    let component = if target == b"qemu-x86_64-monitor" {
        bexos_secure_monitor_abi::firmware::HYPERVISOR
    } else {
        bexos_secure_monitor_abi::firmware::TRUSTY
    };
    let transaction = firmware::stage_and_activate(
        image,
        request.core.generation,
        component,
        activation,
        || {
            if let Some(transport) = ctx.transport.as_mut() {
                transport.pump_storage()?;
            }
            Ok(())
        },
    )?;
    let report = transaction.report();
    if component == bexos_secure_monitor_abi::firmware::TRUSTY && activation == ACTIVATE_LIVE_NOW {
        #[cfg(target_arch = "aarch64")]
        if report.outcome == bexos_secure_monitor_abi::firmware::APPLYING {
            if let Err(status) = rebind_live_sessions(ctx, true) {
                log_status("candidate transport rebind", status);
                let _ = transaction.resolve(false);
                let _ = rebind_live_sessions(ctx, false);
                return Err(if status == STATUS_INVALID_ARGS {
                    STATUS_PEER_CLOSED
                } else {
                    status
                });
            }
            if let Err(status) = probe_candidate_services(
                ctx,
                request.core.generation,
                report.cutover_started_ms,
                Some(&transaction),
            ) {
                log_status("candidate readiness", status);
                let _ = transaction.resolve(false);
                let _ = rebind_live_sessions(ctx, false);
                return Err(STATUS_VERIFY_FAILED);
            }
            let committed = transaction.resolve(true).inspect_err(|status| {
                log_status("candidate durable commit", *status);
            })?;
            if committed.outcome != bexos_secure_monitor_abi::firmware::COMMITTED {
                return Err(STATUS_UNAVAILABLE);
            }
            ctx.transport
                .as_mut()
                .ok_or(STATUS_UNAVAILABLE)?
                .fence_storage_writes(false);
            syscall::log(
                "tee-driver-trusty: candidate services accepted and public sessions rebound to committed transport generation\n",
            );
            return Ok(committed);
        }
        if report.outcome == bexos_secure_monitor_abi::firmware::COMMITTED {
            rebind_live_sessions(ctx, false)?;
            syscall::log(
                "tee-driver-trusty: public sessions rebound to committed transport generation\n",
            );
        }
    }
    Ok(report)
}

fn set_update_failed(ctx: &mut Context, generation: u64, phase: u32) {
    ctx.update.status = 5;
    ctx.update.phase = phase;
    ctx.update.generation = generation;
    ctx.update.pending_slot = [0; 16];
    ctx.update.reboot_required = 0;
}

pub(crate) fn secure_monitor(
    x0: u64,
    x1: u64,
    x2: u64,
    x3: u64,
    x4: u64,
    x5: u64,
    x6: u64,
    x7: u64,
) -> Result<[u64; 8], i32> {
    let request = SecureMonitorCallRequest {
        x0,
        x1,
        x2,
        x3,
        x4,
        x5,
        x6,
        x7,
    };
    let decoded: SecureMonitorCallResponse =
        bexos_userspace::ipc::kernel_call(9, "Call", SECURE_MONITOR_PUBLIC_METHODS, &request)
            .map_err(kernel_status)?;
    if decoded.status != kernel_fidl::Status::Ok {
        return Err(kernel_status(decoded.status));
    }
    Ok([
        decoded.x0, decoded.x1, decoded.x2, decoded.x3, decoded.x4, decoded.x5, decoded.x6,
        decoded.x7,
    ])
}

fn secure_monitor_registers(regs: [u64; 8]) -> Result<[u64; 8], i32> {
    secure_monitor(
        regs[0], regs[1], regs[2], regs[3], regs[4], regs[5], regs[6], regs[7],
    )
}

fn monitor_ok(regs: [u64; 8]) -> Result<u64, i32> {
    match regs[0] as i64 {
        0 => Ok(regs[1]),
        code => Err(monitor_status(code)),
    }
}

fn monitor_status(status: i64) -> i32 {
    match status {
        x if x == MonitorStatus::InvalidArgs as i64 => STATUS_INVALID_ARGS,
        x if x == MonitorStatus::AccessDenied as i64 => STATUS_ACCESS_DENIED,
        x if x == MonitorStatus::Unsupported as i64 => STATUS_UNAVAILABLE,
        x if x == MonitorStatus::NoResources as i64 => STATUS_RESOURCE_EXHAUSTED,
        x if x == MonitorStatus::InvalidHandle as i64 => STATUS_INVALID_ARGS,
        x if x == MonitorStatus::Busy as i64 => STATUS_RESOURCE_EXHAUSTED,
        _ => STATUS_UNAVAILABLE,
    }
}

pub(crate) fn kernel_status(status: kernel_fidl::Status) -> i32 {
    match status {
        kernel_fidl::Status::Ok => STATUS_OK,
        kernel_fidl::Status::ErrInvalidArgs | kernel_fidl::Status::ErrInvalidHandle => {
            STATUS_INVALID_ARGS
        }
        kernel_fidl::Status::ErrAccessDenied => STATUS_ACCESS_DENIED,
        kernel_fidl::Status::ErrNoMemory => STATUS_NO_MEMORY,
        kernel_fidl::Status::ErrBufferTooSmall => STATUS_BUFFER_TOO_SMALL,
        kernel_fidl::Status::ErrPeerClosed => STATUS_PEER_CLOSED,
        kernel_fidl::Status::ErrTimedOut => STATUS_TIMED_OUT,
        kernel_fidl::Status::ErrAlreadyExists => STATUS_ALREADY_EXISTS,
        kernel_fidl::Status::ErrResourceExhausted => STATUS_RESOURCE_EXHAUSTED,
    }
}

fn slot_name(bytes: &[u8]) -> [u8; 16] {
    let mut slot = [0; 16];
    slot[..bytes.len().min(16)].copy_from_slice(&bytes[..bytes.len().min(16)]);
    slot
}
