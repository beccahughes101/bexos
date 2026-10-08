use bexos_appd::runner::deferred::Deferred;
use bexos_appd::runner::kernel::NativeRunnerPrepareRequest;
use bexos_appd::{
    ComponentRunnerProviderKind, FakeKernelOps, HardwareAccessTier, KernelHandle, KernelOperation,
    KernelOps,
};

#[test]
fn deferred_start_closes_construction_handles_exactly_once() {
    for fail_first_close in [false, true] {
        let mut kernel = FakeKernelOps::new();
        let mut deferred = Deferred::new(&mut kernel);
        let process = deferred
            .create_process(
                "replacement",
                1,
                "bexos.service.scened",
                HardwareAccessTier::None,
                true,
            )
            .unwrap();
        let arena = deferred
            .create_sub_vmar(process.root_vmar, 0, 4096, 0)
            .unwrap();
        if fail_first_close {
            deferred.kernel.fail_call(deferred.kernel.call_count() + 1);
        }
        assert_eq!(deferred.close_handle(arena.vmar).is_err(), fail_first_close);
        deferred.close_handle(process.root_vmar).unwrap();
        deferred
            .start_thread_in_process(process.process, process.address_space, 4096, 8192, 0, None)
            .unwrap();
        let thread = deferred.start().unwrap();
        assert!(kernel.is_handle_live(thread));
        for handle in [process.root_vmar, arena.vmar] {
            assert_eq!(
                kernel
                    .operations
                    .iter()
                    .filter(|op| { **op == KernelOperation::CloseHandle { handle } })
                    .count(),
                1,
                "closed construction handles must not survive guard cleanup"
            );
            assert!(!kernel.is_handle_live(handle));
        }
    }
}

#[test]
fn deferred_forwards_native_runner_preparation() {
    let mut kernel = FakeKernelOps::new();
    let prepared = {
        let mut deferred = Deferred::new(&mut kernel);
        deferred
            .prepare_native_runner(
                KernelHandle { raw: 10 },
                &NativeRunnerPrepareRequest {
                    provider_kind: ComponentRunnerProviderKind::ComponentRunner,
                    provider_package: "bexos.platform.wasm_runner",
                    provider_path: "/pkg/bin/wasm_runner",
                    provider_package_dir: Some(KernelHandle { raw: 11 }),
                    provider_image: None,
                    provider_image_size: 0,
                    dependencies: &[],
                    dependency_images: &[],
                    dependency_image_sizes: &[],
                    target_process: KernelHandle { raw: 12 },
                    target_address_space: KernelHandle { raw: 13 },
                    target_root_vmar: KernelHandle { raw: 14 },
                    target_job: KernelHandle { raw: 15 },
                    runner: KernelHandle { raw: 16 },
                    events: KernelHandle { raw: 17 },
                    events_reply: KernelHandle { raw: 18 },
                },
            )
            .unwrap()
    };
    assert!(!prepared.main_thread.is_none());
    assert!(kernel.operations.iter().any(|operation| matches!(
        operation,
        KernelOperation::NativeRunnerPrepare { provider_package, .. }
            if provider_package == "bexos.platform.wasm_runner"
    )));
}
