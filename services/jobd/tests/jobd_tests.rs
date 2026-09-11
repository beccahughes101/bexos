use bexos_job_store::{
    ClockSnapshot, JobConstraints, JobSpec, NetworkRequirement, RuntimeConditions,
};
use bexos_jobd::service::{JobdService, SchedulerClient};

fn client() -> SchedulerClient {
    SchedulerClient {
        channel: 1,
        package_id: "com.example.app".into(),
        uid: 42,
        allowed_ordinals: (1 << 1) | (1 << 2) | (1 << 3) | (1 << 4),
    }
}

fn declaration() -> JobSpec {
    JobSpec {
        job_id: "sync".into(),
        target_component: "worker".into(),
        initial_delay_seconds: 0,
        interval_seconds: 3600,
        flex_window_seconds: 300,
        constraints: JobConstraints {
            network: NetworkRequirement::Unmetered,
            require_charging: true,
            require_device_idle: true,
            require_battery_not_low: true,
        },
        max_execution_seconds: 30,
        persist_across_reboots: true,
    }
}

#[test]
fn scheduler_clamps_to_registered_manifest_declarations() {
    let mut service = JobdService::new();
    service.register_declarations("com.example.app", 9, vec![declaration()]);

    assert!(
        service
            .schedule(
                &client(),
                declaration(),
                ClockSnapshot {
                    monotonic_seconds: 0,
                    realtime_seconds: Some(0),
                },
            )
            .is_ok()
    );

    let mut too_fast = declaration();
    too_fast.interval_seconds = 5;
    assert!(
        service
            .schedule(&client(), too_fast, ClockSnapshot::default())
            .is_err()
    );
}

#[test]
fn due_jobs_respect_runtime_conditions() {
    let mut service = JobdService::new();
    service.register_declarations("com.example.app", 9, vec![declaration()]);
    service
        .schedule(
            &client(),
            declaration(),
            ClockSnapshot {
                monotonic_seconds: 0,
                realtime_seconds: Some(0),
            },
        )
        .unwrap();

    assert_eq!(service.due_jobs(0, RuntimeConditions::default()).len(), 1);
    assert_eq!(
        service
            .due_jobs(
                0,
                RuntimeConditions {
                    network_unmetered: false,
                    ..RuntimeConditions::default()
                },
            )
            .len(),
        0
    );
}
