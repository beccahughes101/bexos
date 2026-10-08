use super::*;
use bexos_appd::PackageDirectories;

struct NixImage {
    bytes: Vec<u8>,
}

impl PackageImageResolver for NixImage {
    fn supports_directory_payloads(&self) -> bool {
        true
    }

    fn component_directories(
        &self,
        package: &str,
    ) -> Result<PackageDirectories, PackageImageError> {
        if package == "bexos.platform.starnix_runner" {
            Ok(PackageDirectories::default())
        } else {
            Ok(PackageDirectories {
                package_dir: Some(KernelHandle { raw: 73 }),
                dependencies: Vec::new(),
            })
        }
    }

    fn resolve_executable<'a>(
        &'a self,
        package_name: &str,
        path: &str,
    ) -> Result<PackageImage<'a>, PackageImageError> {
        assert!(
            (package_name == "bexos.platform.starnix_fixture" && path == "/pkg/bin/hello")
                || (package_name == "bexos.platform.starnix_runner"
                    && path == "/pkg/bin/starnix_runner")
                || (package_name == "bexos.platform.native_runner"
                    && path == "/pkg/bin/native_runner")
        );
        if path == "/pkg/bin/native_runner" {
            return Ok(PackageImage {
                bytes: Box::leak(valid_elf_with_tls().into_boxed_slice()),
                vmo: KernelHandle { raw: 72 },
                vmo_offset: 0,
            });
        }
        Ok(PackageImage {
            bytes: &self.bytes,
            vmo: KernelHandle { raw: 70 },
            vmo_offset: 0,
        })
    }
}

struct ForgedNixImage(NixImage);

impl PackageImageResolver for ForgedNixImage {
    fn resolve_executable<'a>(
        &'a self,
        package_name: &str,
        path: &str,
    ) -> Result<PackageImage<'a>, PackageImageError> {
        self.0.resolve_executable(package_name, path)
    }
}

fn manifest() -> Manifest {
    Manifest::decode(include_bytes!(env!("NIX_TEST_MANIFEST"))).expect("Starnix manifest")
}

fn launch(kernel: &mut FakeKernelOps) -> Result<bexos_appd::LaunchResult, LaunchError> {
    let manifest = manifest();
    let mut runner_policy = bexos_appd::RunnerPolicy::default();
    runner_policy.allow_starnix_runner = true;
    runner_policy.component_runner_providers = vec![bexos_appd::ComponentRunnerProvider {
        runner_name: "nix".into(),
        kind: bexos_appd::ComponentRunnerProviderKind::ComponentRunner,
        package_id: "bexos.platform.starnix_runner".into(),
        executable_path: "/pkg/bin/starnix_runner".into(),
        expected_signer: "bexos_official_platform_v1".into(),
    }];
    RunnerRegistry::new().launch(
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
        kernel,
        &NixImage { bytes: valid_elf() },
    )
}

#[test]
fn nix_launch_rejects_a_missing_provider_registry_before_process_creation() {
    let manifest = manifest();
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
        &ForgedNixImage(NixImage { bytes: valid_elf() }),
    );
    assert_eq!(result, Err(LaunchError::RunnerPolicyDenied));
    assert!(kernel.operations.is_empty());
}

#[test]
fn prototxt_nix_options_decode_with_stable_fields() {
    let manifest = manifest();
    let process = &manifest.processes[0];
    assert_eq!(process.runner, "nix");
    let Some(ProcessRunnerOptions::Nix(options)) = &process.runner_options else {
        panic!("missing Nix options")
    };
    assert_eq!(options.path, "/pkg/bin/hello");
    assert_eq!(options.arguments, ["hello"]);
    assert_eq!(options.environment.len(), 1);
    assert_eq!(options.environment[0].name, "LANG");
    assert_eq!(options.environment[0].value, "C");
}

#[test]
fn nix_launch_uses_authenticated_runtime_and_versioned_startup() {
    let mut kernel = FakeKernelOps::new();
    let result = launch(&mut kernel).expect("Nix launch");
    assert!(kernel.is_handle_live(result.process_handle));
    let startup = kernel
        .operations
        .iter()
        .find_map(|operation| match operation {
            KernelOperation::ComponentStart {
                runner,
                program_type_url,
                program,
                ..
            } if runner == "nix" => Some((program_type_url, program)),
            _ => None,
        });
    let Some((type_url, bytes)) = startup else {
        panic!("missing Nix startup record")
    };
    assert_eq!(type_url, bexos_starnix_abi::OPTIONS_TYPE_URL);
    let decoded = bexos_starnix_abi::NixRunnerOptions::decode(bytes).expect("runner options");
    assert_eq!(decoded.path, "/pkg/bin/hello");
    assert!(kernel.operations.iter().any(|operation| matches!(
        operation,
        KernelOperation::MapInVmSpace {
            target_vaddr: TEST_TLS_BASE,
            requested_rights,
            ..
        } if *requested_rights
            == bexos_kernel_core::loader::RIGHTS_READ
                | bexos_kernel_core::loader::RIGHTS_WRITE
    )));
    assert!(kernel.operations.iter().any(|operation| matches!(
        operation,
        KernelOperation::StartThreadInProcess {
            thread_pointer_vaddr,
            ..
        } if *thread_pointer_vaddr == TEST_TLS_BASE
            + if bexos_app_manifest::Architecture::current_guest()
                == bexos_app_manifest::Architecture::X86_64
            {
                4096
            } else {
                0
            }
    )));
}

#[test]
fn nix_partial_launch_failures_release_every_owned_handle() {
    let mut successful = FakeKernelOps::new();
    launch(&mut successful).expect("control launch");
    let calls = successful.call_count();
    assert!(calls > 4);

    for call in 1..=calls {
        let mut kernel = FakeKernelOps::new();
        kernel.fail_call(call);
        let result = launch(&mut kernel);
        assert!(
            result.is_err(),
            "failure point {call} unexpectedly launched: {:?}",
            kernel.operations
        );
        assert_eq!(
            kernel.live_handle_count(),
            0,
            "failure point {call} leaked handles: {:?}",
            kernel.operations
        );
    }
}

#[test]
fn restricted_kick_is_exposed_through_kernel_operations() {
    let mut kernel = FakeKernelOps::new();
    bexos_appd::KernelOps::kick_restricted_thread(&mut kernel, KernelHandle { raw: 41 })
        .expect("restricted kick");
    assert_eq!(
        kernel.operations,
        [KernelOperation::KickRestrictedThread {
            thread: KernelHandle { raw: 41 },
        }]
    );
}
