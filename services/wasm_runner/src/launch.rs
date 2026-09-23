use crate::{
    executor::block_on,
    host::{NativeHost, entry},
};
use bexos_userspace::{Channel, Memory, Startup};
use bexos_wasm_abi::Launch;
use bexos_wasm_runtime::{
    budget::Budget,
    context::Context,
    resources::{Kind, Origin, READ, TRANSFER, WRITE},
};
use std::sync::Arc;
use wasmtime::{Result, bail};
struct ModuleMapping {
    handle: u64,
    address: u64,
    len: u64,
}
impl Drop for ModuleMapping {
    fn drop(&mut self) {
        let _ = Memory::unmap(self.address, self.len);
        let _ = Memory::close(self.handle);
    }
}

fn map_payload(handle: u64, len: u64, label: &str) -> Result<Arc<[u8]>> {
    let address = match Memory::map(handle, len, 2) {
        Ok(v) => v,
        Err(e) => {
            let _ = Memory::close(handle);
            bail!("{label} mapping: {e:?}");
        }
    };
    let mapping = ModuleMapping {
        handle,
        address,
        len,
    };
    let bytes: Arc<[u8]> =
        unsafe { std::slice::from_raw_parts(mapping.address as *const u8, mapping.len as usize) }
            .into();
    drop(mapping);
    Ok(bytes)
}

pub fn run(channel: Channel) -> Result<u8> {
    let message = channel
        .recv_blocking()
        .map_err(|e| wasmtime::format_err!("runner prelude: {e:?}"))?;
    let launch = match Launch::decode(&message.bytes) {
        Ok(v) => v,
        Err(e) => {
            for h in message.handles {
                let _ = Memory::close(h);
            }
            bail!("invalid runner options: {e:?}");
        }
    };
    if message.handles.len() != launch.component_dependencies.len() + 1 {
        for h in message.handles {
            let _ = Memory::close(h);
        }
        bail!("runner prelude payload count mismatch");
    }
    // The immutable, private copies are the exact inputs validated by appd and
    // compiled below. Dependency VMOs carry only component bytes, never handles.
    let bytes = map_payload(message.handles[0], launch.module_len, "payload")?;
    let mut dependency_bytes = Vec::with_capacity(launch.component_dependencies.len());
    for (index, dependency) in launch.component_dependencies.iter().enumerate() {
        dependency_bytes.push(map_payload(
            message.handles[index + 1],
            dependency.module_len,
            "dependency payload",
        )?);
    }
    let mut startup =
        Startup::receive(channel).map_err(|e| wasmtime::format_err!("startup: {e:?}"))?;
    bexos_libc::install_startup(&startup);
    let engine = bexos_wasm_runtime::engine::configured_engine(&launch.options.limits)?;
    let component_payloads: Vec<_> = launch
        .component_dependencies
        .iter()
        .cloned()
        .zip(dependency_bytes.iter().cloned())
        .map(
            |(dependency, bytes)| bexos_wasm_runtime::migration::ComponentPayload {
                dependency,
                bytes,
            },
        )
        .collect();
    let bytes = crate::composition::compose_application(
        bytes,
        &launch.component_dependencies,
        dependency_bytes,
        launch.options.limits.max_module_bytes,
    )?;
    let budget = Budget::for_limits(&launch.options.limits);
    let host = Arc::new(NativeHost::new());
    if startup.migration_target {
        if !launch.service {
            bail!("commands use restart lifecycle");
        }
        let migration = channel;
        let runtime = crate::migration::receive(
            engine,
            bytes,
            launch.options,
            host,
            migration,
            startup.migration_generation,
        )?;
        return crate::service::serve(runtime);
    }
    host.locale.lock().unwrap().install(startup.locale.take())?;
    let mut context = Context::new(launch.options, host.clone(), Origin::Signed, budget);
    context.component_dependencies = component_payloads;
    crate::command::install(&mut context, &startup)?;
    for ns in &startup.namespace {
        let (_, rights) = bexos_userspace::Memory::object_info(ns.directory)
            .map_err(|e| wasmtime::format_err!("namespace rights: {e:?}"))?;
        let admitted = (if rights & 2 != 0 { READ } else { 0 })
            | (if rights & 4 != 0 { WRITE } else { 0 })
            | (if rights & 1 != 0 { TRANSFER } else { 0 });
        context.resources.insert(entry(
            ns.path.clone(),
            ns.directory,
            Kind::Directory,
            admitted,
        ))?;
    }
    for grant in &startup.service_grants {
        let entry = bexos_wasm_runtime::resources::Entry {
            name: grant.service.clone(),
            handle: Arc::new(crate::host::NativeHandle {
                raw: grant.endpoint,
                kind: Kind::Channel,
                rights: READ | WRITE | TRANSFER,
                companions: Vec::new(),
                allowed_methods: None,
                ownership: None,
                grant: Some(bexos_wasm_runtime::resources::Grant {
                    service: grant.service.clone(),
                    protocol: grant.protocol.clone(),
                    capability: grant.capability.clone(),
                    method_ordinals: grant.method_ordinals.clone(),
                    permission_values: grant.permission_values.clone(),
                    caller_package: grant.caller_package.clone(),
                    caller_uid: grant.caller_uid,
                    caller_foreground: grant.caller_foreground,
                }),
            }),
        };
        host.observe_grant(&entry);
        context.resources.insert(entry)?;
    }
    if launch.service {
        bexos_userspace::log(&format!(
            "wasm_runner: compiling service component bytes={}\n",
            bytes.len()
        ));
        let instance = block_on(
            bexos_wasm_runtime::service_guest::ServiceGuest::instantiate(&engine, bytes, context),
        )?;
        bexos_userspace::log("wasm_runner: service component instantiated\n");
        return crate::service::fresh(instance, channel, startup.migration, host);
    }
    if bytes.get(4..8) == Some(&[1, 0, 0, 0]) {
        let mut instance = block_on(bexos_wasm_runtime::instance::CoreInstance::instantiate(
            &engine, bytes, context,
        ))?;
        Startup::ready(channel).map_err(|e| wasmtime::format_err!("ready: {e:?}"))?;
        block_on(instance.run())?;
        Ok(0)
    } else {
        let mut instance = block_on(bexos_wasm_runtime::component::CommandInstance::instantiate(
            &engine, bytes, context,
        ))?;
        Startup::ready(channel).map_err(|e| wasmtime::format_err!("ready: {e:?}"))?;
        block_on(instance.run())
    }
}
