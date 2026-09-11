use crate::arch::ArchAPI;
use bexos_kernel_core::runtime::Context;

pub fn yield_now(frame: *mut Context) {
    bexos_trace::trace_instant!(
        bexos_trace::CATEGORY_KERNEL_SCHED,
        "kernel:schedule_voluntary_yield"
    );
    if frame.is_null() {
        return;
    }
    let frame = unsafe { &mut *frame };
    if !crate::arch::CurrentArch::is_userspace(frame) {
        return;
    }
    if crate::arch::CurrentArch::current_cpu_id() != 0 {
        timer_tick(frame);
        return;
    }
    crate::userspace::RUNTIME.with(|s| {
        if let Some(rt) = s.as_mut() {
            crate::memory::reclamation::maintain(rt);
            let address_space =
                rt.schedule_yield_at_on_cpu(0, crate::arch::CurrentArch::monotonic_ns(), frame);
            crate::arch::CurrentArch::switch_address_space(address_space);
            crate::arch::CurrentArch::program_scheduler_deadline(rt.next_deadline_on_cpu(0));
        }
    });
}

pub fn timer_tick(frame: *mut Context) {
    bexos_trace::trace_instant!(
        bexos_trace::CATEGORY_KERNEL_SCHED,
        "kernel:schedule_timer_tick"
    );
    if frame.is_null() {
        return;
    }
    let frame = unsafe { &mut *frame };
    if !crate::arch::CurrentArch::is_userspace(frame) {
        return;
    }
    crate::userspace::RUNTIME.with(|s| {
        if let Some(rt) = s.as_mut() {
            let cpu_id = crate::arch::CurrentArch::current_cpu_id() as u8;
            let executing = rt.scheduler.current_on_cpu(cpu_id).map(|t| t.id);
            rt.scheduler.account_runtime(crate::arch::CurrentArch::monotonic_ns());
            let now = crate::migration::now_ms();
            let handover = rt.handover.as_ref().map(|handover| (
                handover.session.generation(), handover.session.phase(),
                handover.source, handover.target,
                handover.session.cutover_started_ms(),
            ));
            rt.poll_handover(now);
            if rt.handover.is_none() {
                if let Some((generation, phase, source, target, cutover)) = handover {
                    crate::log_line(&alloc::format!(
                        "service-transplant: deadline rollback generation={generation} phase={phase:?} source={source} candidate={target} cutover_elapsed_ms={:?}",
                        cutover.map(|start| now.saturating_sub(start)),
                    ));
                }
            }
            crate::memory::reclamation::maintain(rt);
            // Handover rollback may already select a different logical owner;
            // the interrupted thread still executes this scheduler epilogue.
            if let Some(executing) = executing {
                rt.scheduler.account_executing(cpu_id, executing, crate::arch::CurrentArch::monotonic_ns());
            }
            bexos_trace::trace_counter!(
                bexos_trace::CATEGORY_KERNEL_SCHED,
                "kernel:schedule_cpu",
                cpu_id as i64
            );
            let address_space =
                rt.schedule_at_on_cpu(cpu_id, crate::arch::CurrentArch::monotonic_ns(), frame);
            crate::arch::CurrentArch::switch_address_space(address_space);
            crate::arch::CurrentArch::program_scheduler_deadline(rt.next_deadline_on_cpu(cpu_id));
        }
    });
}

/// An SGI may interrupt an idle CPU at EL1.  Unlike a normal timer tick that
/// frame has no EL0 state to account, but it is still a valid destination for
/// the next runnable task's saved EL0 context.
pub fn reschedule_ipi(frame: *mut Context) {
    bexos_trace::trace_instant!(
        bexos_trace::CATEGORY_KERNEL_SCHED,
        "kernel:schedule_reschedule_ipi"
    );
    if frame.is_null() {
        return;
    }
    let frame = unsafe { &mut *frame };
    crate::userspace::RUNTIME.with(|s| {
        if let Some(rt) = s.as_mut() {
            let cpu_id = crate::arch::CurrentArch::current_cpu_id() as u8;
            bexos_trace::trace_counter!(
                bexos_trace::CATEGORY_KERNEL_SCHED,
                "kernel:reschedule_cpu",
                cpu_id as i64
            );
            let address_space =
                rt.schedule_at_on_cpu(cpu_id, crate::arch::CurrentArch::monotonic_ns(), frame);
            crate::arch::CurrentArch::switch_address_space(address_space);
            crate::arch::CurrentArch::program_scheduler_deadline(rt.next_deadline_on_cpu(cpu_id));
        }
    });
}
