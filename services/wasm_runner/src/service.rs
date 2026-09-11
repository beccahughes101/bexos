use crate::{
    executor::block_on,
    host::{NativeHandle, NativeHost},
    migration::Runtime,
};
use bexos_userspace::{
    Channel, Memory, Startup,
    live_migration::{RecordChanges, Source},
    service_binding::ServiceBinding,
};
use bexos_wasm_runtime::{
    resources::{Entry, Kind, READ, WRITE},
    service_guest::ServiceGuest,
};
use std::sync::Arc;
use wasmtime::Result;
fn activate(instance: &mut ServiceGuest) -> Result<()> {
    for child in instance.store_mut().data_mut().children.values_mut() {
        match &mut **child {
            bexos_wasm_runtime::child::Child::Core(child) => {
                block_on(child.activate())?;
            }
            bexos_wasm_runtime::child::Child::Service(child) => activate(child)?,
            _ => {}
        }
    }
    block_on(instance.activate())
}
pub fn fresh(
    mut instance: ServiceGuest,
    control: Channel,
    migration: Option<Channel>,
    host: Arc<NativeHost>,
) -> Result<u8> {
    activate(&mut instance)?;
    let mut runtime = Runtime::source(instance, control, migration, host);
    runtime.refresh()?;
    Startup::ready(control).map_err(|e| wasmtime::format_err!("service ready: {e:?}"))?;
    serve(runtime)
}
pub fn serve(mut runtime: Runtime) -> Result<u8> {
    let mut source = Source::new(runtime.migration);
    let mut changes = RecordChanges::default();
    // Activation is called after transport validation and kernel cutover for a
    // candidate; fresh instances have already activated before advertising ready.
    if runtime.instance.as_ref().unwrap().store().data().restoring {
        activate(runtime.instance.as_mut().unwrap())?;
    }
    loop {
        // Checkpointing includes the component bytes and serialized application
        // state. Do it only for a migration request, never on every idle tick.
        let pending = source
            .has_pending()
            .map_err(|error| wasmtime::format_err!("migration endpoint unavailable: {error:?}"))?;
        let checkpoint_ready = if pending && !source.quiescing() {
            let ready = match runtime.refresh() {
                Ok(()) => true,
                Err(error) => {
                    bexos_userspace::log(&format!(
                        "wasm_runner: checkpoint unavailable: {error:#}\n"
                    ));
                    false
                }
            };
            changes.poll(&runtime, &mut source);
            ready
        } else {
            true
        };
        let was_active = source.active();
        if checkpoint_ready {
            if let Err(error) = source.poll_pending(&runtime) {
                bexos_userspace::log(&format!("wasm_runner: migration aborted: {error:?}\n"));
            }
        }
        if was_active && !source.active() {
            block_on(runtime.instance.as_mut().unwrap().abort_migration())?;
        }
        if !source.active() {
            runtime.discard_checkpoint();
        }
        changes.poll(&runtime, &mut source);
        if source.quiescing() {
            bexos_userspace::yield_now();
            continue;
        }
        let mut dispatch = 0;
        if let Ok(message) = runtime.control.try_recv() {
            let binding = std::str::from_utf8(&message.bytes)
                .ok()
                .and_then(ServiceBinding::parse);
            if message.handles.len() == 1 && binding.is_some() {
                let binding = binding.unwrap();
                let entry = Entry {
                    name: format!("client:{}", binding.protocol),
                    handle: Arc::new(NativeHandle {
                        raw: message.handles[0],
                        kind: Kind::Channel,
                        rights: READ | WRITE,
                        companions: Vec::new(),
                        allowed_methods: Some(binding.method_ordinals.clone()),
                        grant: Some(crate::host::grant_metadata(&binding)),
                        ownership: None,
                    }),
                };
                // A rejected insertion drops and closes the received endpoint.
                dispatch = runtime
                    .instance
                    .as_mut()
                    .unwrap()
                    .store_mut()
                    .data_mut()
                    .resources
                    .insert(entry)
                    .unwrap_or(0);
            } else if message.handles.len() == 1
                && message.bytes.starts_with(b"bexos.opener.interface.v1\n")
            {
                let interface = &message.bytes[b"bexos.opener.interface.v1\n".len()..];
                if let Ok(interface) = std::str::from_utf8(interface) {
                    let entry = crate::host::entry(
                        format!("client:{interface}"),
                        message.handles[0],
                        Kind::Channel,
                        READ | WRITE,
                    );
                    dispatch = runtime
                        .instance
                        .as_mut()
                        .unwrap()
                        .store_mut()
                        .data_mut()
                        .resources
                        .insert(entry)
                        .unwrap_or(0);
                } else {
                    let _ = Memory::close(message.handles[0]);
                }
            } else {
                for h in message.handles {
                    let _ = Memory::close(h);
                }
            }
        }
        // A running guest is not a checkpoint. Initial preparation must wait
        // for the safe point above, including after a temporarily busy renderer.
        // Existing transfers may copy their last safe snapshot at fuel yields,
        // but cannot quiesce until dispatch returns.
        runtime.invalidate();
        let mut instance = runtime.instance.take().unwrap();
        let mut aborted = false;
        let result = crate::dispatch::run(&mut instance, dispatch, || {
            let active = source.active();
            if active {
                if let Err(error) = source.poll(&runtime) {
                    bexos_userspace::log(&format!(
                        "wasm_runner: busy migration rejected: {error:?}\n"
                    ));
                }
            }
            aborted |= active && !source.active();
        });
        runtime.instance = Some(instance);
        result?;
        if aborted {
            block_on(runtime.instance.as_mut().unwrap().abort_migration())?;
        }
        bexos_userspace::yield_now();
    }
}
