//! Kernel calls selected by the guest ABI, independent of the build host.
pub use crate::arch::syscall::*;

pub fn exit_with_status(exit_code: i32) -> ! {
    let _ = crate::ipc::kernel_call::<
        kernel_fidl::TaskControlExitThreadRequest,
        kernel_fidl::TaskControlExitThreadResponse,
    >(
        3,
        "ExitThread",
        kernel_fidl::TASK_CONTROL_PUBLIC_METHODS,
        &kernel_fidl::TaskControlExitThreadRequest { exit_code },
    );
    exit()
}

/// Self-only scheduled CPU counters. No heap allocation or process authority.
pub fn runtime_stats() -> Result<(u64, u64), kernel_fidl::Status> {
    let reply: kernel_fidl::TaskControlGetRuntimeStatsResponse = crate::ipc::kernel_call_buffered(
        3,
        "GetRuntimeStats",
        kernel_fidl::TASK_CONTROL_PUBLIC_METHODS,
        &kernel_fidl::TaskControlGetRuntimeStatsRequest {},
        &mut [0; 64],
        &mut [0; 64],
    )?;
    if reply.status == kernel_fidl::Status::Ok {
        Ok((reply.thread_cpu_ns, reply.process_cpu_ns))
    } else {
        Err(reply.status)
    }
}
