use bexos_job_store::{
    ClockSnapshot, JobConstraints, JobRunStatus, JobSpec, JobState, JobTimebase, MemoryJobStore,
    NetworkRequirement, RuntimeConditions, constraints_met, decode_job, encode_job,
};

fn declared() -> Vec<JobSpec> {
    vec![JobSpec {
        job_id: "sync".into(),
        target_component: "worker".into(),
        initial_delay_seconds: 0,
        interval_seconds: 3600,
        flex_window_seconds: 300,
        constraints: JobConstraints {
            network: NetworkRequirement::Any,
            require_charging: false,
            require_device_idle: false,
            require_battery_not_low: true,
        },
        max_execution_seconds: 30,
        persist_across_reboots: true,
    }]
}

#[test]
fn schedules_due_and_completes_periodic_job() {
    let mut store = MemoryJobStore::new();
    let record = store
        .schedule(
            "com.example.app",
            42,
            9,
            declared()[0].clone(),
            &declared(),
            ClockSnapshot {
                monotonic_seconds: 10,
                realtime_seconds: Some(10),
            },
        )
        .unwrap();

    assert_eq!(record.next_run_seconds, 10);
    assert_eq!(store.due_jobs(10, RuntimeConditions::default()).len(), 1);

    store
        .mark_running("com.example.app", 42, "sync", record.job_token)
        .unwrap();
    let completed = store
        .complete_token(record.job_token, true, true, 20)
        .unwrap();
    assert_eq!(completed.state, JobState::Scheduled);
    assert_eq!(completed.last_run_status, JobRunStatus::Ok);
    assert_eq!(completed.next_run_seconds, 3610);
}

#[test]
fn rejects_schedule_that_escalates_past_manifest() {
    let mut store = MemoryJobStore::new();
    let mut requested = declared()[0].clone();
    requested.interval_seconds = 1;

    assert!(
        store
            .schedule(
                "com.example.app",
                42,
                9,
                requested,
                &declared(),
                ClockSnapshot::default()
            )
            .is_err()
    );
}

#[test]
fn constraints_gate_unmetered_and_battery_state() {
    let constraints = JobConstraints {
        network: NetworkRequirement::Unmetered,
        require_charging: true,
        require_device_idle: true,
        require_battery_not_low: true,
    };

    assert!(constraints_met(&constraints, RuntimeConditions::default()));
    assert!(!constraints_met(
        &constraints,
        RuntimeConditions {
            network_unmetered: false,
            ..RuntimeConditions::default()
        }
    ));
    assert!(!constraints_met(
        &constraints,
        RuntimeConditions {
            battery_low: true,
            ..RuntimeConditions::default()
        }
    ));
}

#[test]
fn job_record_round_trips() {
    let mut store = MemoryJobStore::new();
    let record = store
        .schedule(
            "com.example.app",
            7,
            9,
            declared()[0].clone(),
            &declared(),
            ClockSnapshot {
                monotonic_seconds: 99,
                realtime_seconds: Some(99),
            },
        )
        .unwrap();
    assert_eq!(decode_job(&encode_job(&record)).unwrap(), record);
}

#[test]
fn persistent_jobs_wait_for_realtime_anchor() {
    let mut store = MemoryJobStore::new();
    let record = store
        .schedule(
            "com.example.app",
            7,
            9,
            declared()[0].clone(),
            &declared(),
            ClockSnapshot {
                monotonic_seconds: 10,
                realtime_seconds: None,
            },
        )
        .unwrap();
    assert_eq!(record.timebase, JobTimebase::WaitingRealtimeAnchor);
    store.anchor_realtime_jobs(1_000);
    let anchored = store.get("com.example.app", 7, "sync").unwrap();
    assert_eq!(anchored.timebase, JobTimebase::RealtimeUtc);
    assert_eq!(anchored.next_run_seconds, 1_000);
}

#[test]
fn durable_reboot_keeps_only_persistent_and_reschedules_running() {
    let mut store = MemoryJobStore::new();
    let mut transient = declared()[0].clone();
    transient.job_id = "temp".into();
    transient.persist_across_reboots = false;
    transient.interval_seconds = 0;
    let declarations = vec![declared()[0].clone(), transient.clone()];
    let durable = store
        .schedule(
            "com.example.app",
            7,
            9,
            declared()[0].clone(),
            &declarations,
            ClockSnapshot {
                monotonic_seconds: 1,
                realtime_seconds: Some(1),
            },
        )
        .unwrap();
    store
        .schedule(
            "com.example.app",
            7,
            9,
            transient,
            &declarations,
            ClockSnapshot {
                monotonic_seconds: 1,
                realtime_seconds: None,
            },
        )
        .unwrap();
    store
        .mark_running("com.example.app", 7, "sync", durable.job_token)
        .unwrap();
    store.retain_persistent_only();
    store.reconcile_after_boot();
    assert_eq!(store.list_jobs().len(), 1);
    assert_eq!(
        store.get("com.example.app", 7, "sync").unwrap().state,
        JobState::Scheduled
    );
}
