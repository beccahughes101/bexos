//! Core-module ABI. Component bindings use the same checked resource table.
use crate::{
    context::Context,
    resources::{Kind, READ, WRITE},
};
use wasmtime::{Caller, Linker, Memory, Result, bail};
const MAX_MESSAGE: usize = 32768;
fn memory(caller: &mut Caller<'_, Context>) -> Result<Memory> {
    caller
        .get_export("memory")
        .and_then(|e| e.into_memory())
        .ok_or_else(|| wasmtime::format_err!("missing guest memory"))
}
fn read(caller: &mut Caller<'_, Context>, ptr: u32, len: u32) -> Result<Vec<u8>> {
    read_bounded(caller, ptr, len, MAX_MESSAGE)
}
pub(crate) fn read_bounded(
    caller: &mut Caller<'_, Context>,
    ptr: u32,
    len: u32,
    maximum: usize,
) -> Result<Vec<u8>> {
    if len as usize > maximum {
        bail!("hostcall buffer limit");
    }
    let memory = memory(caller)?;
    let range = (ptr as usize)
        ..(ptr as usize)
            .checked_add(len as usize)
            .ok_or_else(|| wasmtime::format_err!("guest range overflow"))?;
    Ok(memory
        .data(&*caller)
        .get(range)
        .ok_or_else(|| wasmtime::format_err!("guest range out of bounds"))?
        .to_vec())
}
pub(crate) fn write(caller: &mut Caller<'_, Context>, ptr: u32, bytes: &[u8]) -> Result<()> {
    memory(caller)?
        .write(caller, ptr as usize, bytes)
        .map_err(Into::into)
}
pub fn link(linker: &mut Linker<Context>) -> Result<()> {
    linker.func_wrap(
        "bexos:kernel/time@1.0.0",
        "monotonic-ns",
        |caller: Caller<'_, Context>| -> u64 { caller.data().host.monotonic_ns() },
    )?;
    linker.func_wrap(
        "bexos:kernel/ipc@1.0.0",
        "resource-find",
        |mut caller: Caller<'_, Context>, ptr: u32, len: u32| -> Result<u32> {
            let bytes = read(&mut caller, ptr, len)?;
            let name = std::str::from_utf8(&bytes)?;
            Ok(caller.data().resources.find(name).unwrap_or(0))
        },
    )?;
    linker.func_wrap(
        "bexos:kernel/ipc@1.0.0",
        "resource-close",
        |mut caller: Caller<'_, Context>, id: u32| -> Result<()> {
            caller.data_mut().resources.remove(id)?;
            Ok(())
        },
    )?;
    linker.func_wrap(
        "bexos:kernel/ipc@1.0.0",
        "channel-write",
        |mut caller: Caller<'_, Context>,
         id: u32,
         ptr: u32,
         len: u32,
         hptr: u32,
         hcount: u32|
         -> Result<i32> {
            if caller.data().restoring {
                bail!("external I/O during restoration");
            }
            if hcount > 16 {
                bail!("handle transfer limit");
            }
            let bytes = read(&mut caller, ptr, len)?;
            let ids = read(&mut caller, hptr, hcount * 4)?;
            let ids: Vec<_> = ids
                .chunks_exact(4)
                .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
                .collect();
            let mut entries = Vec::new();
            for (index, h) in ids.iter().enumerate() {
                if ids[..index].contains(h) {
                    bail!("duplicate handle transfer");
                }
                entries.push(caller.data().resources.transferable(*h)?.clone());
            }
            let channel = caller
                .data()
                .resources
                .get(id, Kind::Channel, WRITE)?
                .handle
                .clone();
            if caller
                .data()
                .host
                .channel_write(&*channel, &bytes, &entries)
                .is_err()
            {
                return Ok(-1);
            }
            for id in ids {
                caller.data_mut().resources.remove(id)?;
            }
            Ok(0)
        },
    )?;
    linker.func_wrap(
        "bexos:kernel/ipc@1.0.0",
        "channel-read",
        |mut caller: Caller<'_, Context>,
         id: u32,
         ptr: u32,
         capacity: u32,
         hptr: u32,
         hcapacity: u32|
         -> Result<i64> {
            if caller.data().restoring {
                bail!("external I/O during restoration");
            }
            if capacity as usize > MAX_MESSAGE || hcapacity > 16 {
                bail!("receive limit");
            }
            // Validate output spans before consuming a queued message.
            read(&mut caller, ptr, capacity)?;
            read(&mut caller, hptr, hcapacity * 4)?;
            if caller.data().resources.remaining() < hcapacity as usize {
                bail!("resource table full");
            }
            let channel = caller
                .data()
                .resources
                .get(id, Kind::Channel, READ)?
                .handle
                .clone();
            let host = caller.data().host.clone();
            let result = caller.data_mut().resources.receive(hcapacity as usize, || {
                let (bytes, handles) =
                    host.channel_read(&*channel, capacity as usize, hcapacity as usize)?;
                if bytes.len() > capacity as usize {
                    bail!("host exceeded receive bytes");
                }
                Ok((bytes, handles))
            });
            let Ok((bytes, resources)) = result else {
                return Ok(-1);
            };
            let mut ids = Vec::new();
            for id in resources {
                ids.extend_from_slice(&id.to_le_bytes());
            }
            write(&mut caller, ptr, &bytes)?;
            write(&mut caller, hptr, &ids)?;
            Ok(((ids.len() as i64 / 4) << 32) | bytes.len() as i64)
        },
    )?;
    linker.func_wrap(
        "bexos:wasm/sandbox@1.0.0",
        "parent-send",
        |mut caller: Caller<'_, Context>, ptr: u32, len: u32| -> Result<()> {
            let bytes = read(&mut caller, ptr, len)?;
            let messages = &mut caller.data_mut().messages;
            if messages.len() >= 16 {
                bail!("parent message queue full");
            }
            messages.push_back(bytes);
            Ok(())
        },
    )?;
    crate::sandbox::link(linker)?;
    Ok(())
}
