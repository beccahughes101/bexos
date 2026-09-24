use super::*;

struct NixImage {
    bytes: Vec<u8>,
}

impl PackageImageResolver for NixImage {
    fn resolve_executable<'a>(
        &'a self,
        package_name: &str,
        path: &str,
    ) -> Result<PackageImage<'a>, PackageImageError> {
        assert_eq!(package_name, "bexos.platform.starnix_fixture");
        assert_eq!(path, "/pkg/bin/hello");
        Ok(PackageImage {
            bytes: &self.bytes,
            vmo: KernelHandle { raw: 70 },
            vmo_offset: 0,
        })
    }
}

struct ForgedNixImage(NixImage);

impl PackageImageResolver for ForgedNixImage {
    fn starnix_runtime_digest(&self) -> [u8; 32] {
        [0; 32]
    }

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
    RunnerRegistry::new().launch(
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
        kernel,
        &NixImage { bytes: valid_elf() },
    )
}

#[test]
fn nix_launch_rejects_a_runtime_digest_mismatch_before_process_creation() {
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
            KernelOperation::RunnerStartup { bytes, module, .. } => Some((bytes, module)),
            _ => None,
        });
    let Some((bytes, payload)) = startup else {
        panic!("missing Nix startup record")
    };
    let decoded = bexos_starnix_abi::Launch::decode(bytes).expect("startup ABI");
    assert_eq!(decoded.options.path, "/pkg/bin/hello");
    assert_eq!(decoded.image_len, valid_elf().len() as u64);
    assert!(!decoded.service);
    assert!(!decoded.migratable);
    assert!(!kernel.is_handle_live(*payload));
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
            "failure point {call} unexpectedly launched"
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
