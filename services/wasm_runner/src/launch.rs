use crate::{
    executor::block_on,
    host::{NativeHost, entry},
};
use bexos_userspace::{Channel, Memory, Startup};
use bexos_wasm_abi::{ComponentDependency, Launch, WasmRunnerOptions};
use bexos_wasm_runtime::{
    budget::Budget,
    context::Context,
    resources::{Kind, Origin, READ, TRANSFER, WRITE},
};
use std::sync::Arc;
use wasmtime::{Result, bail};
struct DirectoryPayloads {
    package_dir: Option<u64>,
    package_image: Option<(u64, u64)>,
    dependencies: Vec<bexos_component_runner::ResolvedDependency>,
}

impl Drop for DirectoryPayloads {
    fn drop(&mut self) {
        if let Some(package_dir) = self.package_dir {
            let _ = Memory::close(package_dir);
        }
        if let Some((package_image, _)) = self.package_image {
            let _ = Memory::close(package_image);
        }
        for dependency in &self.dependencies {
            let _ = Memory::close(dependency.directory);
        }
    }
}

fn validate_wasm(bytes: Vec<u8>, label: &str) -> Result<Arc<[u8]>> {
    if bytes.get(..4) != Some(b"\0asm") {
        bail!("{label} is not a WebAssembly module or component");
    }
    Ok(bytes.into())
}
pub fn run(channel: Channel) -> Result<u8> {
    let start = bexos_component_runner::receive_start(
        channel,
        "wasm",
        bexos_wasm_abi::OPTIONS_TYPE_URL,
        false,
    )
    .map_err(|error| wasmtime::format_err!("component runner start: {error:?}"))?;
    let options = WasmRunnerOptions::decode(&start.program)
        .map_err(|error| wasmtime::format_err!("invalid runner options: {error:?}"))?;
    let startup_channel = start.startup;
    let service = start.service;
    let migratable = start.migratable;
    let directories = DirectoryPayloads {
        package_dir: start.package_dir,
        package_image: start.package_image,
        dependencies: start.dependencies,
    };
    let package_bytes = match (directories.package_dir, directories.package_image) {
        (Some(package_dir), None) => bexos_component_runner::read_package_file(
            package_dir,
            &options.path,
            options.limits.max_module_bytes,
        ),
        (None, Some((package_image, package_image_size))) => {
            bexos_component_runner::read_package_image(
                package_image,
                package_image_size,
                options.limits.max_module_bytes,
            )
        }
        _ => Err(bexos_component_runner::Error::Payloads),
    }
    .map_err(|error| wasmtime::format_err!("package payload: {error:?}"))?;
    let bytes = validate_wasm(package_bytes, "payload")?;
    let mut dependency_bytes = Vec::with_capacity(options.component_imports.len());
    let mut component_dependencies = Vec::with_capacity(options.component_imports.len());
    for import in &options.component_imports {
        let resolved = directories
            .dependencies
            .iter()
            .find(|resolved| {
                resolved.kind == bexos_component_runner::DependencyKind::WasmComponent
                    && resolved.package_name == import.package_name
                    && resolved.export_name == import.export_name
                    && resolved.abi_version == import.abi_version
            })
            .ok_or_else(|| wasmtime::format_err!("missing resolved component dependency"))?;
        let dependency = validate_wasm(
            bexos_component_runner::read_package_file(
                resolved.directory,
                &resolved.export_path,
                options.limits.max_module_bytes,
            )
            .map_err(|error| wasmtime::format_err!("dependency payload: {error:?}"))?,
            "dependency payload",
        )?;
        component_dependencies.push(ComponentDependency {
            import: import.clone(),
            module_len: dependency.len() as u64,
        });
        dependency_bytes.push(dependency);
    }
    let launch = Launch {
        options,
        module_len: bytes.len() as u64,
        service,
        migratable,
        component_dependencies,
    };
    launch
        .validate()
        .map_err(|error| wasmtime::format_err!("invalid launch metadata: {error:?}"))?;
    let mut startup =
        Startup::receive(startup_channel).map_err(|e| wasmtime::format_err!("startup: {e:?}"))?;
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
        let migration = startup_channel;
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
        return crate::service::fresh(instance, startup_channel, startup.migration, host);
    }
    if bytes.get(4..8) == Some(&[1, 0, 0, 0]) {
        let mut instance = block_on(bexos_wasm_runtime::instance::CoreInstance::instantiate(
            &engine, bytes, context,
        ))?;
        Startup::ready(startup_channel).map_err(|e| wasmtime::format_err!("ready: {e:?}"))?;
        let _ = bexos_component_runner::ready();
        block_on(instance.run())?;
        Ok(0)
    } else {
        let mut instance = block_on(bexos_wasm_runtime::component::CommandInstance::instantiate(
            &engine, bytes, context,
        ))?;
        Startup::ready(startup_channel).map_err(|e| wasmtime::format_err!("ready: {e:?}"))?;
        let _ = bexos_component_runner::ready();
        block_on(instance.run())
    }
}
