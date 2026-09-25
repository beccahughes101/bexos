use super::*;
struct WasmImages {
    trusted: bool,
    component: Option<PackageLibraryKind>,
}

const DIOXUS_COMPONENT_PACKAGE: &str = "com.bexos.lib.dioxus";
const DIOXUS_COMPONENT_EXPORT: &str = "bexos:wasm/dioxus@1.0.0";

impl PackageImageResolver for WasmImages {
    fn resolve_executable<'a>(
        &'a self,
        _package: &str,
        path: &str,
    ) -> Result<PackageImage<'a>, PackageImageError> {
        let bytes: &[u8] = if path == bexos_wasm_abi::RUNNER_PATH {
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
fn wasm_launch_keeps_consumer_identity_and_sends_payload_prelude() {
    let mut manifest = manifest();
    manifest.architecture = bexos_app_manifest::Architecture::Multi;
    let mut kernel = FakeKernelOps::new();
    let result = RunnerRegistry::new()
        .launch(
            &LaunchRequest {
                manifest: &manifest,
                process: &manifest.processes[0],
                trust_tier: PackageTrustTier::StandardConsumer,
                identity: package_identity(&manifest, PackageTrustTier::StandardConsumer, false),
                runner_policy: None,
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
    assert!(kernel.operations.iter().any(|op|matches!(op,KernelOperation::CreateProcess{package_id,resource_group_id:9,hardware_access:HardwareAccessTier::None,..} if package_id==&manifest.package_name)));
    assert!(
        kernel
            .operations
            .iter()
            .any(|op| matches!(op, KernelOperation::RunnerStartupHandles { modules, .. } if modules.len() == 1))
    );
}

#[test]
fn wasm_launch_sends_declared_component_dependency_payloads() {
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
    let mut kernel = FakeKernelOps::new();
    RunnerRegistry::new()
        .launch(
            &LaunchRequest {
                manifest: &manifest,
                process: &manifest.processes[0],
                trust_tier: PackageTrustTier::StandardConsumer,
                identity: package_identity(&manifest, PackageTrustTier::StandardConsumer, false),
                runner_policy: None,
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
        KernelOperation::RunnerStartupHandles { bytes, modules, .. } => Some((bytes, modules)),
        _ => None,
    });
    let Some((bytes, modules)) = startup else {
        panic!("missing startup")
    };
    assert_eq!(modules.len(), 2);
    let launch = bexos_wasm_abi::Launch::decode(bytes).unwrap();
    assert_eq!(launch.component_dependencies.len(), 1);
    assert_eq!(
        launch.component_dependencies[0].import.package_name,
        "com.bexos.lib.dioxus"
    );
}

#[test]
fn wasm_launch_rejects_native_payload_substitution_for_component_import() {
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
    let mut kernel = FakeKernelOps::new();
    let result = RunnerRegistry::new().launch(
        &LaunchRequest {
            manifest: &manifest,
            process: &manifest.processes[0],
            trust_tier: PackageTrustTier::StandardConsumer,
            identity: package_identity(&manifest, PackageTrustTier::StandardConsumer, false),
            runner_policy: None,
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
    assert_eq!(
        result,
        Err(LaunchError::PackageImage(PackageImageError::AccessDenied))
    );
    assert!(kernel.operations.is_empty());
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
fn replacement_runtime_requires_its_own_platform_signature_and_fixed_role() {
    use bexos_appd::runner::runtime_archive;
    let archive = include_bytes!(env!("WASM_TEST_RUNTIME_ARCHIVE"));
    let verified = runtime_archive::verify(archive).unwrap();
    assert!(verified.bytes.starts_with(b"\x7fELF"));
    assert_ne!(
        verified.bytes.as_slice(),
        include_bytes!(env!("WASM_TEST_RUNTIME"))
    );
    let mut tampered = archive.to_vec();
    let middle = tampered.len() / 2;
    tampered[middle] ^= 1;
    assert!(runtime_archive::verify(&tampered).is_err());
    assert!(runtime_archive::verify(include_bytes!(env!("WASM_TEST_COMMAND_ARCHIVE"))).is_err());
    assert!(runtime_archive::verify(include_bytes!(env!("WASM_TEST_RUNTIME"))).is_err());
}

#[test]
fn failed_standard_startup_releases_the_native_wasm_process() {
    let launch = bexos_appd::LaunchResult {
        process_handle: KernelHandle { raw: 1 },
        address_space_handle: KernelHandle { raw: 2 },
        main_thread_handle: KernelHandle { raw: 3 },
        service_manager_handle: KernelHandle { raw: 4 },
        runtime_linker_data: None,
    };
    let mut kernel = FakeKernelOps::new();
    {
        let _pending = bexos_appd::runner::PendingWasmLaunch::new(&mut kernel, Some(launch));
    }
    assert!(kernel.operations.iter().any(
        |op| matches!(op, KernelOperation::TerminateProcess { process, .. } if process.raw == 1)
    ));
    for raw in 1..=4 {
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
