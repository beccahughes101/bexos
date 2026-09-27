use super::*;
use bexos_appd::{PackageDirectories, PackageDirectoryDependency};
struct WasmImages {
    trusted: bool,
    component: Option<PackageLibraryKind>,
}

const DIOXUS_COMPONENT_PACKAGE: &str = "com.bexos.lib.dioxus";
const DIOXUS_COMPONENT_EXPORT: &str = "bexos:wasm/dioxus@1.0.0";

impl PackageImageResolver for WasmImages {
    fn supports_directory_payloads(&self) -> bool {
        true
    }

    fn component_directories(
        &self,
        package: &str,
    ) -> Result<PackageDirectories, PackageImageError> {
        if package == "bexos.platform.wasm_runner" {
            return Ok(PackageDirectories::default());
        }
        let dependencies = self
            .component
            .map(|kind| PackageDirectoryDependency {
                package_name: DIOXUS_COMPONENT_PACKAGE.into(),
                mount_alias: DIOXUS_COMPONENT_PACKAGE.into(),
                export_name: DIOXUS_COMPONENT_EXPORT.into(),
                export_path: "/pkg/lib/dioxus.wasm".into(),
                symbol_prefix: String::new(),
                soname: String::new(),
                abi_version: 1,
                kind,
                direct_dependencies: Vec::new(),
                directory: KernelHandle { raw: 74 },
            })
            .into_iter()
            .collect();
        Ok(PackageDirectories {
            package_dir: Some(KernelHandle { raw: 73 }),
            dependencies,
        })
    }

    fn resolve_executable<'a>(
        &'a self,
        _package: &str,
        path: &str,
    ) -> Result<PackageImage<'a>, PackageImageError> {
        let bytes: &[u8] = if path == "/pkg/bin/native_runner" {
            return Ok(PackageImage {
                bytes: Box::leak(valid_elf().into_boxed_slice()),
                vmo: KernelHandle { raw: 72 },
                vmo_offset: 0,
            });
        } else if path == bexos_wasm_abi::RUNNER_PATH {
            if self.trusted {
                include_bytes!(env!("WASM_TEST_RUNTIME"))
            } else {
                b"\x7fELFforged"
            }
        } else {
            b"\0asm\x01\0\0\0"
        };
        Ok(PackageImage {
            bytes,
            vmo: KernelHandle { raw: 70 },
            vmo_offset: 0,
        })
    }
    fn resolve_wasm_component<'a>(
        &'a self,
        package_name: &str,
        export_name: &str,
        abi_version: u32,
    ) -> Result<PackageLibrary<'a>, PackageImageError> {
        if package_name != DIOXUS_COMPONENT_PACKAGE
            || export_name != DIOXUS_COMPONENT_EXPORT
            || abi_version != 1
        {
            return Err(PackageImageError::NotFound);
        }
        static DIRECT: [PackageLibraryDependency; 0] = [];
        Ok(PackageLibrary {
            package_name: DIOXUS_COMPONENT_PACKAGE,
            export_name: DIOXUS_COMPONENT_EXPORT,
            soname: "",
            image: PackageImage {
                bytes: b"\0asm\x0d\0\0\0",
                vmo: KernelHandle { raw: 71 },
                vmo_offset: 0,
            },
            symbol_prefix: "",
            abi_version,
            kind: self.component.ok_or(PackageImageError::NotFound)?,
            direct_dependencies: &DIRECT,
        })
        .and_then(|library| {
            if library.kind == PackageLibraryKind::WasmComponent {
                Ok(library)
            } else {
                Err(PackageImageError::AccessDenied)
            }
        })
    }
}
fn manifest() -> Manifest {
    Manifest::decode(include_bytes!(env!("WASM_TEST_MANIFEST"))).unwrap()
}

fn wasm_runner_policy() -> bexos_appd::RunnerPolicy {
    let mut policy = bexos_appd::RunnerPolicy::default();
    policy.component_runner_providers = vec![bexos_appd::ComponentRunnerProvider {
        runner_name: "wasm".into(),
        kind: bexos_appd::ComponentRunnerProviderKind::ComponentRunner,
        package_id: "bexos.platform.wasm_runner".into(),
        executable_path: "/pkg/bin/wasm_runner".into(),
        expected_signer: "bexos_official_platform_v1".into(),
    }];
    policy
}

#[test]
fn prototxt_wasm_options_decode() {
    let m = manifest();
    assert_eq!(m.processes[0].runner, "wasm");
    let Some(ProcessRunnerOptions::Wasm(options)) = &m.processes[0].runner_options else {
        panic!("missing WASM options")
    };
    assert_eq!(options.path, "/pkg/bin/command.wasm");
    assert_eq!(options.limits, bexos_wasm_abi::Limits::default());
}
#[test]
fn wasm_launch_keeps_consumer_identity_and_forwards_opaque_program() {
    let mut manifest = manifest();
    manifest.architecture = bexos_app_manifest::Architecture::Multi;
    let mut kernel = FakeKernelOps::new();
    let runner_policy = wasm_runner_policy();
    let result = RunnerRegistry::new()
        .launch(
            &LaunchRequest {
                manifest: &manifest,
                process: &manifest.processes[0],
                trust_tier: PackageTrustTier::StandardConsumer,
                identity: package_identity(&manifest, PackageTrustTier::StandardConsumer, false),
                runner_policy: Some(&runner_policy),
                hardware_access: HardwareAccessTier::None,
                realtime_scheduling: false,
                resource_group_id: 9,
            },
            &mut kernel,
            &WasmImages {
                trusted: true,
                component: None,
            },
        )
        .unwrap();
    assert!(!result.process_handle.is_none());
    assert!(kernel.operations.iter().any(|op|matches!(op,KernelOperation::CreateComponentJob{package_id,resource_group_id:9,hardware_access:HardwareAccessTier::None,max_processes:1,..} if package_id==&manifest.package_name)));
    assert!(kernel.operations.iter().any(
        |op| matches!(op, KernelOperation::ComponentStart { runner, program_type_url, program, .. }
                if runner == "wasm"
                    && program_type_url == bexos_wasm_abi::OPTIONS_TYPE_URL
                    && bexos_wasm_abi::WasmRunnerOptions::decode(program).is_ok())
    ));
}

#[test]
fn wasm_launch_forwards_declared_component_imports_without_reading_payloads() {
    let mut manifest = manifest();
    let Some(ProcessRunnerOptions::Wasm(options)) = &mut manifest.processes[0].runner_options
    else {
        panic!("missing WASM options")
    };
    options
        .component_imports
        .push(bexos_wasm_abi::ComponentImport {
            package_name: "com.bexos.lib.dioxus".into(),
            export_name: "bexos:wasm/dioxus@1.0.0".into(),
            abi_version: 1,
            instance_name: "bexos:wasm/dioxus@1.0.0".into(),
        });
    manifest.processes[0].runner_program = None;
    let mut kernel = FakeKernelOps::new();
    let runner_policy = wasm_runner_policy();
    RunnerRegistry::new()
        .launch(
            &LaunchRequest {
                manifest: &manifest,
                process: &manifest.processes[0],
                trust_tier: PackageTrustTier::StandardConsumer,
                identity: package_identity(&manifest, PackageTrustTier::StandardConsumer, false),
                runner_policy: Some(&runner_policy),
                hardware_access: HardwareAccessTier::None,
                realtime_scheduling: false,
                resource_group_id: 9,
            },
            &mut kernel,
            &WasmImages {
                trusted: true,
                component: Some(PackageLibraryKind::WasmComponent),
            },
        )
        .unwrap();
    let startup = kernel.operations.iter().find_map(|op| match op {
        KernelOperation::ComponentStart {
            runner,
            program_type_url,
            program,
            ..
        } if runner == "wasm" => Some((program_type_url, program)),
        _ => None,
    });
    let Some((type_url, bytes)) = startup else {
        panic!("missing startup")
    };
    assert_eq!(type_url, bexos_wasm_abi::OPTIONS_TYPE_URL);
    let options = bexos_wasm_abi::WasmRunnerOptions::decode(bytes).unwrap();
    assert_eq!(options.component_imports.len(), 1);
    assert_eq!(
        options.component_imports[0].package_name,
        "com.bexos.lib.dioxus"
    );
}

#[test]
fn wasm_launch_leaves_dependency_format_validation_to_the_provider() {
    let mut manifest = manifest();
    let Some(ProcessRunnerOptions::Wasm(options)) = &mut manifest.processes[0].runner_options
    else {
        panic!("missing WASM options")
    };
    options
        .component_imports
        .push(bexos_wasm_abi::ComponentImport {
            package_name: "com.bexos.lib.dioxus".into(),
            export_name: "bexos:wasm/dioxus@1.0.0".into(),
            abi_version: 1,
            instance_name: "bexos:wasm/dioxus@1.0.0".into(),
        });
    manifest.processes[0].runner_program = None;
    let mut kernel = FakeKernelOps::new();
    let runner_policy = wasm_runner_policy();
    let result = RunnerRegistry::new().launch(
        &LaunchRequest {
            manifest: &manifest,
            process: &manifest.processes[0],
            trust_tier: PackageTrustTier::StandardConsumer,
            identity: package_identity(&manifest, PackageTrustTier::StandardConsumer, false),
            runner_policy: Some(&runner_policy),
            hardware_access: HardwareAccessTier::None,
            realtime_scheduling: false,
            resource_group_id: 9,
        },
        &mut kernel,
        &WasmImages {
            trusted: true,
            component: Some(PackageLibraryKind::Native),
        },
    );
    assert!(result.is_ok());
    assert!(kernel.operations.iter().any(|operation| matches!(
        operation,
        KernelOperation::ComponentStart { runner, .. } if runner == "wasm"
    )));
}
#[test]
fn wasm_cannot_select_a_forged_runner_or_launch_as_driver() {
    for (trusted, driver) in [(false, false), (true, true)] {
        let manifest = manifest();
        let mut kernel = FakeKernelOps::new();
        let result = RunnerRegistry::new().launch(
            &LaunchRequest {
                manifest: &manifest,
                process: &manifest.processes[0],
                trust_tier: PackageTrustTier::StandardConsumer,
                identity: package_identity(&manifest, PackageTrustTier::StandardConsumer, driver),
                runner_policy: None,
                hardware_access: HardwareAccessTier::None,
                realtime_scheduling: false,
                resource_group_id: 9,
            },
            &mut kernel,
            &WasmImages {
                trusted,
                component: None,
            },
        );
        assert_eq!(result, Err(LaunchError::RunnerPolicyDenied));
        assert!(kernel.operations.is_empty());
    }
}

#[test]
fn component_owned_runner_override_is_rejected() {
    assert!(
        bexos_appd::runner::runtime_archive::contains_component_override(include_bytes!(env!(
            "WASM_TEST_NESTED_RUNNER"
        )))
    );
}

#[test]
fn failed_standard_startup_releases_the_native_wasm_process() {
    let launch = bexos_appd::LaunchResult {
        job_handle: KernelHandle { raw: 5 },
        process_handle: KernelHandle { raw: 1 },
        address_space_handle: KernelHandle { raw: 2 },
        main_thread_handle: KernelHandle { raw: 3 },
        service_manager_handle: KernelHandle { raw: 4 },
        controller_handle: KernelHandle::none(),
        events_handle: KernelHandle::none(),
        runtime_linker_data: None,
        native_host: None,
    };
    let mut kernel = FakeKernelOps::new();
    {
        let _pending = bexos_appd::runner::PendingWasmLaunch::new(&mut kernel, Some(launch));
    }
    assert!(
        kernel
            .operations
            .iter()
            .any(|op| matches!(op, KernelOperation::TerminateJob { job, .. } if job.raw == 5))
    );
    for raw in 1..=5 {
        assert!(
            kernel.operations.iter().any(
                |op| matches!(op, KernelOperation::CloseHandle { handle } if handle.raw == raw)
            )
        );
    }
    let mut kernel = FakeKernelOps::new();
    {
        let mut pending = bexos_appd::runner::PendingWasmLaunch::new(&mut kernel, Some(launch));
        pending.ready();
    }
    assert!(kernel.operations.is_empty());
}
