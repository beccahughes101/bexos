use super::*;
use crate::manifest::{ElfRunnerOptions, ProcessRunnerOptions, UpdateStrategy};
use bexos_starnix_abi::{Launch, RUNNER_PATH};

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
    let Some(ProcessRunnerOptions::Nix(options)) = &request.process.runner_options else {
        return Err(LaunchError::MissingRunnerOptions);
    };
    options
        .validate()
        .map_err(|_| LaunchError::InvalidNixOptions)?;
    let migratable = request.process.lifecycle.update_strategy == UpdateStrategy::HeartTransplant;
    if request.process.service != migratable {
        // A Phase 2 service must be transplantable: restart-only Linux
        // services would violate the platform service lifecycle contract.
        return Err(LaunchError::InvalidNixOptions);
    }
    let payload = resolver
        .resolve_executable(&request.manifest.package_name, &options.path)
        .map_err(LaunchError::PackageImage)?;
    // The runner resolves PT_INTERP inside the selected Linux rootfs. Appd only
    // authenticates and validates the main executable here; rejecting dynamic
    // images at this boundary made ordinary glibc/musl programs impossible.
    starnix_kernel::validate_executable(payload.bytes, starnix_kernel::Architecture::current())
        .map_err(|_| LaunchError::InvalidNixOptions)?;

    let runtime = resolver
        .resolve_starnix_runtime()
        .map_err(LaunchError::PackageImage)?;
    let runtime_bytes = runtime.bytes;
    use sha2::Digest;
    if sha2::Sha256::digest(runtime_bytes).as_slice() != resolver.starnix_runtime_digest() {
        return Err(LaunchError::RunnerPolicyDenied);
    }
    let runtime_vmo = kernel
        .create_vmo_from_bytes(runtime_bytes)
        .map_err(|source| LaunchError::Kernel {
            operation: "starnix_runtime_vmo",
            source,
        })?;
    let launch_result = (|| {
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
        let image = RuntimeImage {
            bytes: runtime_bytes,
            vmo: runtime_vmo,
        };
        let result = elf.launch_trusted_runtime(&native, kernel, &image)?;
        let payload_vmo = match kernel.create_vmo_from_bytes(payload.bytes) {
            Ok(vmo) => vmo,
            Err(source) => {
                cleanup(kernel, &result);
                return Err(LaunchError::Kernel {
                    operation: "starnix_payload_vmo",
                    source,
                });
            }
        };
        let prelude = Launch {
            options: options.clone(),
            image_len: payload.bytes.len() as u64,
            service: request.process.service,
            migratable,
        }
        .encode()
        .map_err(|_| LaunchError::InvalidNixOptions)?;
        if let Err(source) =
            kernel.send_runner_startup(result.service_manager_handle, &prelude, payload_vmo)
        {
            let _ = kernel.release_vmo(payload_vmo);
            cleanup(kernel, &result);
            return Err(LaunchError::Kernel {
                operation: "starnix_startup",
                source,
            });
        }
        let _ = kernel.release_vmo(payload_vmo);
        Ok(result)
    })();
    let _ = kernel.release_vmo(runtime_vmo);
    launch_result
}

struct RuntimeImage<'a> {
    bytes: &'a [u8],
    vmo: KernelHandle,
}

impl PackageImageResolver for RuntimeImage<'_> {
    fn resolve_executable<'a>(
        &'a self,
        _package: &str,
        path: &str,
    ) -> Result<PackageImage<'a>, PackageImageError> {
        if path != RUNNER_PATH {
            return Err(PackageImageError::AccessDenied);
        }
        Ok(PackageImage {
            bytes: self.bytes,
            vmo: self.vmo,
            vmo_offset: 0,
        })
    }
}

fn cleanup<K: KernelOps>(kernel: &mut K, result: &LaunchResult) {
    let _ = kernel.terminate_process(result.process_handle, -1);
    if let Some((handle, _)) = result.runtime_linker_data {
        let _ = kernel.release_vmo(handle);
    }
    let _ = kernel.close_handle(result.service_manager_handle);
    let _ = kernel.close_handle(result.main_thread_handle);
    let _ = kernel.close_handle(result.address_space_handle);
    let _ = kernel.close_handle(result.process_handle);
}
