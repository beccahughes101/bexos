//! Checked C ABI leases for mapped and device-only Venus allocations.
use super::*;
#[repr(C)]
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct Memory {
    resource: u32,
    host_visible: u32,
    size: u64,
    address: usize,
    lease: u64,
}
pub(super) struct Lease {
    pub context: u32,
    info: Memory,
    mapping: Option<rt::Mapping>,
}
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn bexos_venus_memory_create(
    context: u32,
    size: u64,
    blob_id: u64,
    map: u32,
    out: *mut Memory,
) -> i32 {
    if out.is_null() || out.addr() % core::mem::align_of::<Memory>() != 0 || map > 1 {
        return INVALID;
    }
    let Some(size) = size
        .checked_add(4095)
        .map(|n| n & !4095)
        .filter(|n| *n != 0 && *n <= bexos_virtio_gpu_protocol::transport::MAX_RESOURCE_BYTES)
    else {
        bexos_userspace::log(&format!(
            "venus-transport: allocation exceeds resource limit context={context} bytes={size}\n"
        ));
        return NO_MEMORY;
    };
    with(|t| {
        if !t.contexts.iter().any(|c| c.id == context) {
            return Err(INVALID);
        }
        if t.memory.len() == 64 {
            bexos_userspace::log("venus-transport: allocation reached 64 process leases\n");
            return Err(NO_MEMORY);
        }
        let lease = t.next_lease;
        t.next_lease = lease.checked_add(1).ok_or(NO_MEMORY)?;
        let r: DisplayCoordinatorCreateGpuBlobResponse = rt::call(
            &mut t.channel,
            15,
            &DisplayCoordinatorCreateGpuBlobRequest {
                context,
                size,
                blob_id,
                host_visible: map != 0,
            },
        )
        .map_err(|_| DEVICE_LOST)?;
        if let Err(error) = status(r.status) {
            bexos_userspace::log(&format!(
                "venus-transport: allocation rejected context={context} bytes={size} mapped={map} status={:?} leases={} process_bytes={}\n",
                r.status,
                t.memory.len(),
                t.memory.iter().map(|m| m.info.size).sum::<u64>()
            ));
            if let Some(m) = r.memory {
                rt::close(&[m.buffer.raw]);
            }
            return Err(error);
        }
        let mut mapping = None;
        let valid = match r.memory {
            Some(m) if map != 0 && m.host_visible && m.buffer.raw != 0 => {
                mapping = match rt::Mapping::map(m.buffer.raw, size, 6) {
                    Ok(mapping) => Some(mapping),
                    Err(error) => {
                        bexos_userspace::log(&format!(
                            "venus-transport: VMO mapping failed bytes={size} error={error:?}\n"
                        ));
                        None
                    }
                };
                mapping.is_some()
            }
            Some(m) => {
                rt::close(&[m.buffer.raw]);
                false
            }
            None => map == 0,
        };
        if !valid || r.resource < 3 || t.memory.iter().any(|m| m.info.resource == r.resource) {
            drop(mapping);
            let _ = t
                .rpc
                .call::<_, DisplayCoordinatorReleaseGpuResourceResponse>(
                    &mut t.channel,
                    14,
                    &DisplayCoordinatorReleaseGpuResourceRequest {
                        context,
                        resource: r.resource,
                    },
                );
            return Err(DEVICE_LOST);
        }
        let info = Memory {
            resource: r.resource,
            host_visible: map,
            size,
            address: mapping.as_ref().map_or(0, |m| m.address as usize),
            lease,
        };
        t.memory.push(Lease {
            context,
            info,
            mapping,
        });
        unsafe {
            out.write(info);
        }
        Ok(())
    })
}
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn bexos_venus_memory_release(
    context: u32,
    memory: *mut Memory,
) -> i32 {
    if memory.is_null() || memory.addr() % core::mem::align_of::<Memory>() != 0 {
        return INVALID;
    }
    let info = unsafe { memory.read() };
    with(|t| {
        let index = t
            .memory
            .iter()
            .position(|m| m.context == context && m.info == info)
            .ok_or(INVALID)?;
        if let Some(mapping) = &mut t.memory[index].mapping {
            if mapping.address != 0 {
                bexos_userspace::Memory::unmap(mapping.address, mapping.size)
                    .map_err(|_| DEVICE_LOST)?;
                // Retain the handle separately if closing fails. Retrying must
                // never unmap a virtual address that may have been reused.
                mapping.address = 0;
                mapping.owned = false;
            }
            bexos_userspace::Memory::close(mapping.handle).map_err(|_| DEVICE_LOST)?;
        }
        t.memory[index].mapping = None;
        let r: DisplayCoordinatorReleaseGpuResourceResponse = t
            .rpc
            .call(
                &mut t.channel,
                14,
                &DisplayCoordinatorReleaseGpuResourceRequest {
                    context,
                    resource: info.resource,
                },
            )
            .map_err(|_| DEVICE_LOST)?;
        status(r.status)?;
        t.memory.swap_remove(index);
        unsafe {
            memory.write(Memory::default());
        }
        Ok(())
    })
}
