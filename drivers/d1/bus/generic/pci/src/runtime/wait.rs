//! Sleep between discovery deadlines while waking immediately for control IPC.
use super::state::Runtime;
use kernel_fidl::*;
pub fn idle(state: &Runtime, quiescing: bool) {
    let now = bexos_userspace::live_migration::now_ms();
    let deadline =
        if !quiescing && state.extended && state.registry.0 != 0 && state.pending.is_none() {
            state.next_poll_ms.max(now)
        } else {
            now.saturating_add(250)
        };
    let mut items = [InlineVectorStruct1 {
        h: HandleRef { raw: 0 },
        signals: Signals(Signals::READABLE.0 | Signals::PEER_CLOSED.0),
    }; 11];
    let channels = core::iter::once(state.control.manager)
        .chain(state.control.migration)
        .chain((state.pending.is_some()).then_some(state.registry))
        .chain(state.power.iter().copied());
    let mut count = 0;
    for channel in channels {
        if channel.0 != 0 {
            items[count].h.raw = channel.0;
            count += 1;
        }
    }
    let _: Result<TaskControlWaitManyResponse, _> = bexos_userspace::ipc::kernel_call_buffered(
        3,
        "WaitMany",
        TASK_CONTROL_PUBLIC_METHODS,
        &TaskControlWaitManyRequest {
            items: WireVector::from_slice(&items[..count]),
            deadline_nanos: deadline.saturating_mul(1_000_000).min(i64::MAX as u64) as i64,
        },
        &mut [0; 512],
        &mut [0; 32],
    );
}
