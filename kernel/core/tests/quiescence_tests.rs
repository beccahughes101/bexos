use bexos_kernel_core::sched::{BlockReason, Scheduler, SchedulerTask, TaskState};

#[test]
fn quiesced_threads_stay_off_cpu_even_when_their_waits_are_woken() {
    let mut scheduler = Scheduler::new();
    for id in 1..=4 {
        let mut task = SchedulerTask::new(id, if id == 4 { 2 } else { 1 }, 1, 1, "test").unwrap();
        if id == 2 || id == 3 {
            task.state = TaskState::Blocked;
            task.block_reason = Some(BlockReason::Futex { uaddr: id * 4096 });
        }
        scheduler.add_task(task).unwrap();
    }
    scheduler.tick();
    scheduler.quiesce_process(1);
    scheduler.wake_task(2).unwrap();
    for _ in 0..32 {
        let decision = scheduler
            .yield_current_to_at_on_cpu(0, None, scheduler.now_ns())
            .unwrap();
        assert_eq!(decision.next_task_id, Some(4));
        for id in 1..=3 {
            assert_eq!(scheduler.task(id).unwrap().state, TaskState::Quiesced);
        }
    }
    assert_eq!(scheduler.task(2).unwrap().block_reason, None);
    assert!(scheduler.task(3).unwrap().block_reason.is_some());
    scheduler.resume_quiesced_process(1);
    assert_eq!(scheduler.task(1).unwrap().state, TaskState::Ready);
    assert_eq!(scheduler.task(2).unwrap().state, TaskState::Ready);
    assert_eq!(scheduler.task(3).unwrap().state, TaskState::Blocked);
    let mut resumed = false;
    for _ in 0..32 {
        let next = scheduler
            .yield_current_to_at_on_cpu(0, None, scheduler.now_ns())
            .unwrap()
            .next_task_id;
        resumed |= matches!(next, Some(1 | 2));
        assert_ne!(next, Some(3));
    }
    assert!(resumed);
}

#[test]
fn stopped_jobs_preserve_waits_and_ignore_wakeups() {
    let mut scheduler = Scheduler::new();
    for id in 1..=3 {
        let mut task = SchedulerTask::new(id, if id == 3 { 2 } else { 1 }, 1, 1, "job").unwrap();
        if id <= 2 {
            task.state = TaskState::Blocked;
            task.block_reason = Some(BlockReason::Futex { uaddr: id * 4096 });
        }
        scheduler.add_task(task).unwrap();
    }
    scheduler.stop_process(1);
    scheduler.wake_task(1).unwrap();
    // Migration quiescence must not erase a user-requested stop.
    scheduler.quiesce_process(1);
    scheduler.resume_quiesced_process(1);
    let mut restored = scheduler;
    for id in 1..=2 {
        assert_eq!(restored.task(id).unwrap().state, TaskState::Stopped);
    }
    assert!(restored.task(1).unwrap().block_reason.is_none());
    assert!(restored.task(2).unwrap().block_reason.is_some());
    restored.resume_stopped_process(1);
    assert_ne!(restored.task(1).unwrap().state, TaskState::Stopped);
    assert_eq!(restored.task(2).unwrap().state, TaskState::Blocked);
}
