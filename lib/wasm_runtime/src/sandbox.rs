//! Child admission and delegation are enforced on every path, independent of
//! guest-provided labels. Signature failures never downgrade to unsigned code.
use crate::{child::Child, context::Context};
use bexos_wasm_abi::WasmRunnerOptions;
use wasmtime::{Caller, Linker, Result, bail};
pub fn link(linker: &mut Linker<Context>) -> Result<()> {
    linker.func_wrap_async(
        "bexos:wasm/sandbox@1.0.0",
        "spawn",
        |mut caller: Caller<'_, Context>,
         (ptr, len, cptr, clen, sptr, slen, gptr, gcount): (
            u32,
            u32,
            u32,
            u32,
            u32,
            u32,
            u32,
            u32,
        )| {
            Box::new(async move {
                if gcount > 16 || caller.data().next_child == u32::MAX {
                    bail!("sandbox admission limit");
                }
                let bytes = crate::abi::read_bounded(&mut caller, ptr, len, 64 << 20)?;
                let config = crate::abi::read_bounded(&mut caller, cptr, clen, 32768)?;
                let signature = crate::abi::read_bounded(&mut caller, sptr, slen, 32768)?;
                let raw_grants = crate::abi::read_bounded(&mut caller, gptr, gcount * 4, 64)?;
                let options = WasmRunnerOptions::decode(&config)
                    .map_err(|e| wasmtime::format_err!("child options: {e:?}"))?;
                let grants = raw_grants
                    .chunks_exact(4)
                    .map(|bytes| u32::from_le_bytes(bytes.try_into().unwrap()))
                    .collect::<Vec<_>>();
                let remaining = caller.get_fuel()?;
                let context = crate::admission::child_context(
                    caller.data(),
                    options.clone(),
                    &bytes,
                    &signature,
                    &grants,
                    remaining,
                )?;
                caller.set_fuel(remaining - options.limits.fuel)?;
                let engine = caller.engine().clone();
                let child = Child::instantiate(&engine, bytes.into(), context).await?;
                let id = caller.data().next_child;
                caller.data_mut().next_child += 1;
                caller.data_mut().children.insert(id, Box::new(child));
                Ok::<u32, wasmtime::Error>(id)
            })
        },
    )?;
    linker.func_wrap_async(
        "bexos:wasm/sandbox@1.0.0",
        "invoke",
        |mut caller: Caller<'_, Context>, (id, ptr, len, input): (u32, u32, u32, i32)| {
            Box::new(async move {
                let bytes = crate::abi::read_bounded(&mut caller, ptr, len, 256)?;
                let name = std::str::from_utf8(&bytes)?;
                let mut child = caller
                    .data_mut()
                    .children
                    .remove(&id)
                    .ok_or_else(|| wasmtime::format_err!("invalid child"))?;
                let result = child.invoke(name, input).await;
                caller.data_mut().children.insert(id, child);
                result
            })
        },
    )?;
    for (name, operation) in [("pause", 1), ("resume", 0), ("terminate", 2)] {
        linker.func_wrap(
            "bexos:wasm/sandbox@1.0.0",
            name,
            move |mut caller: Caller<'_, Context>, id: u32| -> Result<()> {
                let child = caller
                    .data()
                    .children
                    .get(&id)
                    .ok_or_else(|| wasmtime::format_err!("invalid child"))?;
                match operation {
                    1 => child.control().pause(),
                    0 => child.control().resume(),
                    _ => child.control().terminate(),
                }
                if operation == 2 {
                    caller.data_mut().children.remove(&id);
                }
                Ok(())
            },
        )?;
    }
    linker.func_wrap(
        "bexos:wasm/sandbox@1.0.0",
        "receive",
        |mut caller: Caller<'_, Context>, id: u32, ptr: u32, capacity: u32| -> Result<i32> {
            crate::abi::read_bounded(&mut caller, ptr, capacity, 32768)?;
            let child = caller
                .data_mut()
                .children
                .get_mut(&id)
                .ok_or_else(|| wasmtime::format_err!("invalid child"))?;
            let Some(message) = child.store().data().messages.front() else {
                return Ok(-1);
            };
            if message.len() > capacity as usize {
                return Ok(-2);
            }
            let message = child.store_mut().data_mut().messages.pop_front().unwrap();
            crate::abi::write(&mut caller, ptr, &message)?;
            Ok(message.len() as i32)
        },
    )?;
    linker.func_wrap(
        "bexos:wasm/sandbox@1.0.0",
        "status",
        |caller: Caller<'_, Context>, id: u32| -> Result<u32> {
            caller
                .data()
                .child_status(id)
                .map(u32::from)
                .ok_or_else(|| wasmtime::format_err!("invalid child"))
        },
    )?;
    Ok(())
}
