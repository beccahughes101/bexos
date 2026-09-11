use super::*;
use crate::manifest::{ElfRunnerOptions, ProcessRunnerOptions, UpdateStrategy};
use bexos_wasm_abi::{ComponentDependency, Launch, RUNNER_PATH};

pub(super) fn launch<K: KernelOps, R: PackageImageResolver>(
    elf: &ElfRunner,
    request: &LaunchRequest<'_>,
    kernel: &mut K,
    resolver: &R,
) -> Result<LaunchResult, LaunchError> {
    if request.identity.is_driver
        || request.manifest.driver_info.is_some()
        || request.hardware_access != HardwareAccessTier::None
    {
        return Err(LaunchError::RunnerPolicyDenied);
    }
    let Some(ProcessRunnerOptions::Wasm(options)) = &request.process.runner_options else {
        return Err(LaunchError::MissingRunnerOptions);
    };
    options
        .validate()
        .map_err(|_| LaunchError::InvalidWasmOptions)?;
    let migratable = request.process.lifecycle.update_strategy == UpdateStrategy::HeartTransplant;
    if request.process.service && !migratable {
        return Err(LaunchError::InvalidWasmOptions);
    }
    let payload = resolver
        .resolve_executable(&request.manifest.package_name, &options.path)
        .map_err(LaunchError::PackageImage)?;
    if payload.bytes.len() < 8
        || !payload.bytes.starts_with(b"\0asm")
        || payload.bytes.len() as u64 > options.limits.max_module_bytes
    {
        return Err(LaunchError::InvalidWasmOptions);
    }
    let mut component_dependencies = Vec::new();
    for import in &options.component_imports {
        let dependency = resolver
            .resolve_wasm_component(
                &import.package_name,
                &import.export_name,
                import.abi_version,
            )
            .map_err(LaunchError::PackageImage)?;
        if dependency.image.bytes.len() < 8
            || !dependency.image.bytes.starts_with(b"\0asm")
            || dependency.image.bytes.len() as u64 > options.limits.max_module_bytes
        {
            return Err(LaunchError::InvalidWasmOptions);
        }
        component_dependencies.push((import.clone(), dependency.image.bytes));
    }
    let runtime = resolver
        .resolve_wasm_runtime()
        .map_err(LaunchError::PackageImage)?;
    use sha2::Digest;
    if sha2::Sha256::digest(runtime.bytes).as_slice() != resolver.wasm_runtime_digest() {
        return Err(LaunchError::RunnerPolicyDenied);
    }
    let prelude = Launch {
        options: options.clone(),
        module_len: payload.bytes.len() as u64,
        service: request.process.service,
        migratable,
        component_dependencies: component_dependencies
            .iter()
            .map(|(import, bytes)| ComponentDependency {
                import: import.clone(),
                module_len: bytes.len() as u64,
            })
            .collect(),
    }
    .encode()
    .map_err(|_| LaunchError::InvalidWasmOptions)?;
    // Platform code only; never load manifest-declared native libraries into a
    // consumer WASM process. Identity, namespace and scheduling remain its own.
    let mut manifest = request.manifest.clone();
    manifest.library_dependencies.clear();
    let mut process = request.process.clone();
    process.runner = "elf".into();
    process.runner_options = Some(ProcessRunnerOptions::Elf(ElfRunnerOptions {
        path: RUNNER_PATH.into(),
    }));
    let native = LaunchRequest {
        manifest: &manifest,
        process: &process,
        ..*request
    };
    let image = RuntimeImage(runtime);
    let result = elf.launch_trusted_runtime(&native, kernel, &image)?;
    let module = match kernel.create_vmo_from_bytes(payload.bytes) {
        Ok(v) => v,
        Err(source) => {
            cleanup(kernel, &result);
            return Err(LaunchError::Kernel {
                operation: "wasm_payload",
                source,
            });
        }
    };
    let mut modules = Vec::with_capacity(component_dependencies.len() + 1);
    modules.push(module);
    for (_, bytes) in &component_dependencies {
        match kernel.create_vmo_from_bytes(bytes) {
            Ok(vmo) => modules.push(vmo),
            Err(source) => {
                for module in modules {
                    let _ = kernel.release_vmo(module);
                }
                cleanup(kernel, &result);
                return Err(LaunchError::Kernel {
                    operation: "wasm_dependency_payload",
                    source,
                });
            }
        }
    }
    if let Err(source) =
        kernel.send_runner_startup_handles(result.service_manager_handle, &prelude, &modules)
    {
        for module in modules {
            let _ = kernel.release_vmo(module);
        }
        cleanup(kernel, &result);
        return Err(LaunchError::Kernel {
            operation: "wasm_startup",
            source,
        });
    }
    Ok(result)
}
struct RuntimeImage<'a>(PackageImage<'a>);
impl PackageImageResolver for RuntimeImage<'_> {
    fn resolve_executable<'a>(
        &'a self,
        _package: &str,
        path: &str,
    ) -> Result<PackageImage<'a>, PackageImageError> {
        if path != RUNNER_PATH {
            return Err(PackageImageError::AccessDenied);
        }
        Ok(self.0)
    }
}

fn cleanup<K: KernelOps>(kernel: &mut K, result: &LaunchResult) {
    let _ = kernel.terminate_process(result.process_handle, -1);
    if let Some((handle, _)) = result.runtime_linker_data {
        let _ = kernel.release_vmo(handle);
    }
    for handle in [
        result.service_manager_handle,
        result.main_thread_handle,
        result.address_space_handle,
        result.process_handle,
    ] {
        let _ = kernel.close_handle(handle);
    }
}
