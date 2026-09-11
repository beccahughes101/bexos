use bexos_appd::runner::deferred::Deferred;
use bexos_appd::{FakeKernelOps, HardwareAccessTier, KernelOperation, KernelOps};

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
