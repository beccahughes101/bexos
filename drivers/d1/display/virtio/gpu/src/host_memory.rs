//! Host-owned blob mappings. Aperture holes are reusable only after all client
//! VMO references are gone and the host acknowledges unmap and resource unref.
use crate::{
    gpu_state::HostMemory,
    hardware::{put32, put64},
    state::Display,
};
use bexos_userspace::Memory;
use graphics_fidl::Status;

pub fn command(
    d: &mut Display,
    context: u32,
    op: u32,
    expected: u32,
    payload: &[u8],
) -> Result<(), Status> {
    d.hardware
        .as_mut()
        .filter(|h| h.healthy)
        .ok_or(Status::ErrIo)?
        .submit_context_request(op, payload, expected, context, None)
        .map(|_| ())
        .map_err(|_| Status::ErrIo)
}
fn resource_command(d: &mut Display, context: u32, op: u32, id: u32) -> Result<(), Status> {
    let mut payload = [0; 8];
    put32(&mut payload, 0, id);
    command(d, context, op, 0x1100, &payload)
}
fn remove(d: &mut Display, owner: u64, context: u32, id: u32) {
    let _ = d.gpu.registry.remove_resource(owner, context, id);
    d.gpu.host.retain(|m| m.id != id);
}

pub fn create(
    d: &mut Display,
    owner: u64,
    context: u32,
    size: u64,
    blob_id: u64,
    visible: bool,
) -> Result<u32, Status> {
    let mut spans = [(0, 0); 64];
    let mut count = 0;
    for m in d.gpu.host.iter().filter(|m| m.visible) {
        let resource = d
            .gpu
            .registry
            .resource(owner_for(d, m.id)?, context_for(d, m.id)?, m.id)
            .map_err(|_| Status::ErrIo)?;
        spans[count] = (m.offset, resource.size);
        count += 1;
    }
    let offset = if visible {
        d.gpu
            .aperture
            .allocate(size, &spans[..count])
            .map_err(|e| {
                bexos_userspace::log(&format!(
                    "virtio-gpu: aperture allocation failed context={context} bytes={size} aperture_bytes={} mapped_bytes={} mappings={count} error={e:?}\n",
                    d.gpu.aperture.size, spans[..count].iter().map(|s| s.1).sum::<u64>()
                ));
                match e {
                    bexos_virtio_gpu_protocol::Error::Capacity => Status::ErrNoMemory,
                    _ => Status::ErrInvalidArgs,
                }
            })?
    } else {
        0
    };
    let id = d
        .gpu
        .registry
        .create_resource(owner, context, size)
        .map_err(|error| {
            bexos_userspace::log(&format!(
                "virtio-gpu: blob admission failed owner={owner} context={context} bytes={size} resources={} total_bytes={} error={error:?}\n",
                d.gpu.registry.resources.len(),
                d.gpu.registry.resources.iter().map(|r| r.size).sum::<u64>()
            ));
            Status::ErrNoMemory
        })?;
    d.gpu.host.push(HostMemory {
        id,
        visible,
        offset,
        handle: 0,
        mapped: false,
        retiring: false,
        owned: true,
    });
    let mut payload = [0; 32];
    put32(&mut payload, 0, id);
    put32(&mut payload, 4, 2); // HOST3D
    put32(&mut payload, 8, visible as u32); // MAPPABLE only for host-visible allocations.
    put64(&mut payload, 16, blob_id);
    put64(&mut payload, 24, size);
    if let Err(error) = command(d, context, 0x10c, 0x1100, &payload) {
        remove(d, owner, context, id);
        return Err(error);
    }
    Ok(id)
}
fn context_for(d: &Display, id: u32) -> Result<u32, Status> {
    d.gpu
        .registry
        .resources
        .iter()
        .find(|r| r.id == id)
        .map(|r| r.context)
        .ok_or(Status::ErrIo)
}
fn owner_for(d: &Display, id: u32) -> Result<u64, Status> {
    let context = context_for(d, id)?;
    d.gpu
        .registry
        .contexts
        .iter()
        .find(|c| c.id == context)
        .map(|c| c.owner)
        .ok_or(Status::ErrIo)
}
/// Return Some(duplicate VMO) only after creation, mapping, and attachment have
/// completed. A None result submitted exactly one next asynchronous command.
pub fn created(
    d: &mut Display,
    owner: u64,
    context: u32,
    id: u32,
    stage: &mut u8,
    len: usize,
) -> Result<Option<u64>, Status> {
    let index = d
        .gpu
        .host
        .iter()
        .position(|m| m.id == id)
        .ok_or(Status::ErrIo)?;
    match *stage {
        0 => {
            if !d.gpu.host[index].visible {
                resource_command(d, context, 0x202, id)?;
                *stage = 2;
                return Ok(None);
            }
            let mut payload = [0; 16];
            put32(&mut payload, 0, id);
            put64(&mut payload, 8, d.gpu.host[index].offset);
            command(d, 0, 0x208, 0x1106, &payload)?;
            *stage = 1;
            Ok(None)
        }
        1 => {
            d.gpu.host[index].mapped = true;
            let response = unsafe { d.hardware.as_mut().unwrap().command.bytes() };
            if len != 32 || u32::from_le_bytes(response[4120..4124].try_into().unwrap()) & 15 != 1 {
                // Do not silently map WC/uncached memory with cached attributes.
                return Err(Status::ErrIo);
            }
            let resource = d
                .gpu
                .registry
                .resource(owner, context, id)
                .map_err(|_| Status::ErrIo)?;
            let handle = Memory::shared_device(
                d.gpu.aperture.base + d.gpu.host[index].offset,
                resource.size,
            )
            .map_err(|_| Status::ErrIo)?;
            d.gpu.host[index].handle = handle;
            resource_command(d, context, 0x202, id)?;
            *stage = 2;
            Ok(None)
        }
        2 => {
            if !d.gpu.host[index].visible {
                return Ok(Some(0));
            }
            let duplicate = Memory::duplicate(d.gpu.host[index].handle, 1 | 2 | 4 | 16 | 32)
                .map_err(|_| Status::ErrIo)?;
            Ok(Some(duplicate))
        }
        _ => Err(Status::ErrInvalidArgs),
    }
}
pub fn failed_create(d: &mut Display, owner: u64, context: u32, id: u32, stage: u8) {
    if stage == 0 && d.hardware.as_ref().is_some_and(|h| h.healthy) {
        remove(d, owner, context, id);
    } else if let Some(m) = d.gpu.host.iter_mut().find(|m| m.id == id) {
        m.retiring = true;
    }
}
pub fn retire_create(d: &mut Display, id: u32) {
    if let Some(m) = d.gpu.host.iter_mut().find(|m| m.id == id) {
        m.retiring = true;
    }
}
pub fn release(d: &mut Display, context: u32, id: u32) -> Result<(), Status> {
    let memory = d
        .gpu
        .host
        .iter()
        .find(|m| m.id == id)
        .ok_or(Status::ErrInvalidArgs)?;
    if memory.handle != 0
        && !Memory::shared_device_idle(memory.handle).map_err(|_| Status::ErrIo)?
    {
        return Err(Status::ErrBusy);
    }
    resource_command(d, context, 0x203, id)
}
pub fn released(
    d: &mut Display,
    owner: u64,
    context: u32,
    id: u32,
    stage: &mut u8,
) -> Result<bool, Status> {
    let index = d
        .gpu
        .host
        .iter()
        .position(|m| m.id == id)
        .ok_or(Status::ErrIo)?;
    match *stage {
        0 => {
            if d.gpu.host[index].mapped {
                resource_command(d, 0, 0x209, id)?;
                *stage = 1;
            } else {
                resource_command(d, 0, 0x102, id)?;
                *stage = 2;
            }
            Ok(false)
        }
        1 => {
            d.gpu.host[index].mapped = false;
            let handle = core::mem::take(&mut d.gpu.host[index].handle);
            if handle != 0 {
                let _ = Memory::close(handle);
            }
            resource_command(d, 0, 0x102, id)?;
            *stage = 2;
            Ok(false)
        }
        2 => {
            remove(d, owner, context, id);
            Ok(true)
        }
        _ => Err(Status::ErrInvalidArgs),
    }
}
