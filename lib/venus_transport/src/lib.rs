//! Explicit Mesa Venus Vulkan entry points and capability-owned transport.
//! Install an exclusively used granted endpoint before constructing the Vulkan
//! entry. Destroy Vulkan objects and release every context before uninstalling.
//! Renderer pointers never become migration state; the caller drains and rebuilds
//! its process-local Vulkan caches from the committed logical scene.
mod memory;
pub mod probe;
use bexos_graphics_runtime::{self as rt, rpc::Rpc};
use bexos_userspace::Channel;
use core::ffi::{c_char, c_void};
use graphics_fidl::*;
use std::sync::Mutex;

const SUCCESS: i32 = 0;
const NO_MEMORY: i32 = -1;
const INITIALIZATION_FAILED: i32 = -3;
const DEVICE_LOST: i32 = -4;
const INVALID: i32 = -13;
/// Bounded Mesa diagnostics use the service logging path, not Unix stdio.
#[unsafe(no_mangle)]
unsafe extern "C" fn bexos_venus_log(bytes: *const u8, length: usize) {
    if bytes.is_null() || length > 1024 {
        return;
    }
    if let Ok(message) = core::str::from_utf8(unsafe { core::slice::from_raw_parts(bytes, length) })
    {
        bexos_userspace::log("Mesa Venus: ");
        bexos_userspace::log(message);
        bexos_userspace::log("\n");
    }
}
struct Context {
    id: u32,
}
struct Transport {
    channel: Channel,
    capset: Vec<u8>,
    contexts: Vec<Context>,
    memory: Vec<memory::Lease>,
    next_lease: u64,
    rpc: Rpc,
}
static TRANSPORT: Mutex<Option<Transport>> = Mutex::new(None);

/// Takes exclusive use of this endpoint on success; returns it on rejection.
/// The public discovery endpoint remains the caller's responsibility.
pub fn install(channel: Channel, capset: Vec<u8>) -> Result<(), Channel> {
    let Ok(mut slot) = TRANSPORT.lock() else {
        return Err(channel);
    };
    if slot.is_some() || channel.0 == 0 || capset.is_empty() || capset.len() > 4096 {
        return Err(channel);
    }
    let mut contexts = Vec::new();
    let mut memory = Vec::new();
    if contexts.try_reserve_exact(16).is_err() || memory.try_reserve_exact(64).is_err() {
        return Err(channel);
    }
    *slot = Some(Transport {
        channel,
        capset,
        contexts,
        memory,
        next_lease: 1,
        rpc: Rpc::default(),
    });
    Ok(())
}
pub fn uninstall() -> Result<Channel, i32> {
    let mut slot = TRANSPORT.lock().map_err(|_| DEVICE_LOST)?;
    let transport = slot.as_ref().ok_or(INITIALIZATION_FAILED)?;
    if !transport.contexts.is_empty() || !transport.memory.is_empty() {
        return Err(DEVICE_LOST);
    }
    Ok(slot.take().ok_or(INITIALIZATION_FAILED)?.channel)
}
/// Retire process-local mappings after every Vulkan object has been destroyed.
/// Used only after failed teardown; the driver retains uncertain device memory
/// and pins. Closing the returned endpoint orphans those logical resources.
/// No device reset or success acknowledgement is synthesized.
pub fn abandon_after_device_destroyed() -> Option<Channel> {
    TRANSPORT
        .lock()
        .ok()?
        .take()
        .map(|transport| transport.channel)
}

fn with(f: impl FnOnce(&mut Transport) -> Result<(), i32>) -> i32 {
    let Ok(mut slot) = TRANSPORT.lock() else {
        return DEVICE_LOST;
    };
    let Some(t) = slot.as_mut() else {
        return INITIALIZATION_FAILED;
    };
    f(t).map_or_else(|e| e, |_| SUCCESS)
}
fn status(s: Status) -> Result<(), i32> {
    match s {
        Status::Ok => Ok(()),
        Status::ErrNoMemory => Err(NO_MEMORY),
        Status::ErrInvalidArgs => Err(INVALID),
        _ => Err(DEVICE_LOST),
    }
}
unsafe extern "system" {
    fn vn_GetInstanceProcAddr(
        instance: ash::vk::Instance,
        name: *const c_char,
    ) -> ash::vk::PFN_vkVoidFunction;
}
/// Loads the linked Venus implementation explicitly, without dlopen or a Linux
/// ICD manifest and without selecting an unrelated host Vulkan loader.
pub fn entry() -> ash::Entry {
    unsafe {
        ash::Entry::from_static_fn(ash::StaticFn {
            get_instance_proc_addr: vn_GetInstanceProcAddr,
        })
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn bexos_venus_open(
    out: *mut u32,
    capset: *mut c_void,
    capset_size: usize,
) -> i32 {
    if out.is_null() || capset.is_null() || out.addr() % 4 != 0 {
        return INVALID;
    }
    with(|t| {
        if capset_size > 4096 {
            return Err(INVALID);
        }
        if t.contexts.len() == 16 {
            return Err(NO_MEMORY);
        }
        let r: DisplayCoordinatorCreateGpuContextResponse = t
            .rpc
            .call(
                &mut t.channel,
                10,
                &DisplayCoordinatorCreateGpuContextRequest {},
            )
            .map_err(|_| DEVICE_LOST)?;
        status(r.status)?;
        if r.context == 0 || t.contexts.iter().any(|c| c.id == r.context) {
            return Err(DEVICE_LOST);
        }
        t.contexts.push(Context { id: r.context });
        unsafe {
            core::ptr::write_bytes(capset.cast::<u8>(), 0, capset_size);
            core::ptr::copy_nonoverlapping(
                t.capset.as_ptr(),
                capset.cast(),
                t.capset.len().min(capset_size),
            );
            out.write(r.context);
        }
        Ok(())
    })
}
#[unsafe(no_mangle)]
extern "C" fn bexos_venus_close(context: u32) -> i32 {
    with(|t| {
        if !t.contexts.iter().any(|c| c.id == context)
            || t.memory.iter().any(|m| m.context == context)
        {
            return Err(INVALID);
        }
        let r: DisplayCoordinatorDestroyGpuContextResponse = t
            .rpc
            .call(
                &mut t.channel,
                11,
                &DisplayCoordinatorDestroyGpuContextRequest { context },
            )
            .map_err(|_| DEVICE_LOST)?;
        status(r.status)?;
        t.contexts.retain(|c| c.id != context);
        Ok(())
    })
}
#[unsafe(no_mangle)]
unsafe extern "C" fn bexos_venus_submit(
    context: u32,
    ring: u32,
    bytes: *const c_void,
    size: usize,
    completion: *mut u64,
) -> i32 {
    if size > 3992
        || size % 4 != 0
        || (size != 0 && bytes.is_null())
        || completion.is_null()
        || completion.addr() % 8 != 0
    {
        return INVALID;
    }
    let data = if size == 0 {
        &[]
    } else {
        unsafe { core::slice::from_raw_parts(bytes.cast::<u8>(), size) }
    };
    with(|t| {
        if !t.contexts.iter().any(|c| c.id == context) {
            return Err(INVALID);
        }
        let r: DisplayCoordinatorSubmitGpuCommandResponse = t
            .rpc
            .call(
                &mut t.channel,
                12,
                &DisplayCoordinatorSubmitGpuCommandRequest {
                    context,
                    ring,
                    data,
                },
            )
            .map_err(|_| DEVICE_LOST)?;
        status(r.status)?;
        if r.fence == 0 {
            return Err(DEVICE_LOST);
        }
        unsafe {
            completion.write(r.fence);
        }
        Ok(())
    })
}
