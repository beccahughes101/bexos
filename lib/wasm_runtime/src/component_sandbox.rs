//! Component imports use StoreContextMut so fuel reservations and child
//! admission share exactly the same store budget as core-module hostcalls.
use crate::{bindings::bexos::wasm::sandbox::State, child::Child, context::Context};
use wasmtime::{Result, StoreContextMut, component::Linker};
pub fn link(linker: &mut Linker<Context>) -> Result<()> {
    let mut interface = linker.instance("bexos:wasm/sandbox@1.0.0")?;
    interface.func_wrap_async(
        "spawn",
        |mut store: StoreContextMut<'_, Context>,
         (payload, encoded, signature, grants): (Vec<u8>, Vec<u8>, Vec<u8>, Vec<u32>)| {
            Box::new(async move {
                let options = bexos_wasm_abi::WasmRunnerOptions::decode(&encoded)
                    .map_err(|e| wasmtime::format_err!("child options: {e:?}"))?;
                let remaining = store.get_fuel()?;
                let child = crate::admission::child_context(
                    store.data(),
                    options.clone(),
                    &payload,
                    &signature,
                    &grants,
                    remaining,
                )?;
                store.set_fuel(remaining - options.limits.fuel)?;
                let child = Child::instantiate(store.engine(), payload.into(), child).await?;
                let ctx = store.data_mut();
                let id = ctx.next_child;
                ctx.next_child += 1;
                ctx.children.insert(id, Box::new(child));
                Ok((Ok::<u32, ()>(id),))
            })
        },
    )?;
    interface.func_wrap_async(
        "invoke",
        |mut store: StoreContextMut<'_, Context>, (child, name, input): (u32, String, i32)| {
            Box::new(async move {
                if name.len() > 256 {
                    return Ok((Err::<i32, ()>(()),));
                }
                let Some(mut instance) = store.data_mut().children.remove(&child) else {
                    return Ok((Err(()),));
                };
                let result = instance.invoke(&name, input).await;
                store.data_mut().children.insert(child, instance);
                Ok((result.map_err(|_| ()),))
            })
        },
    )?;
    interface.func_wrap(
        "status",
        |store: StoreContextMut<'_, Context>, (id,): (u32,)| {
            Ok((store
                .data()
                .child_status(id)
                .map(|status| match status {
                    0 => State::Running,
                    1 => State::Paused,
                    _ => State::Terminated,
                })
                .ok_or(()),))
        },
    )?;
    for (name, op) in [("pause", 1), ("resume", 0), ("terminate", 2)] {
        interface.func_wrap(
            name,
            move |mut store: StoreContextMut<'_, Context>, (id,): (u32,)| {
                let Some(child) = store.data().children.get(&id) else {
                    return Ok((Err::<(), ()>(()),));
                };
                match op {
                    1 => child.control().pause(),
                    0 => child.control().resume(),
                    _ => child.control().terminate(),
                }
                if op == 2 {
                    store.data_mut().children.remove(&id);
                }
                Ok((Ok(()),))
            },
        )?;
    }
    interface.func_wrap(
        "parent-send",
        |mut store: StoreContextMut<'_, Context>, (message,): (Vec<u8>,)| {
            let ctx = store.data_mut();
            if message.len() > 32768 || ctx.messages.len() >= 16 {
                return Ok((Err::<(), ()>(()),));
            }
            ctx.messages.push_back(message);
            Ok((Ok(()),))
        },
    )?;
    interface.func_wrap(
        "receive",
        |mut store: StoreContextMut<'_, Context>, (id,): (u32,)| {
            Ok((store
                .data_mut()
                .children
                .get_mut(&id)
                .map(|c| c.store_mut().data_mut().messages.pop_front())
                .ok_or(()),))
        },
    )?;
    Ok(())
}
