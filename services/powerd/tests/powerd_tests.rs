use bexos_powerd::migration::Runtime;
use bexos_powerd::{PowerPolicy, TelemetrySample, WakeLease};
use bexos_userspace::Channel;
use bexos_userspace::live_migration::State;
use bexos_userspace::service_binding::BoundServiceEndpoint;
use power_fidl::{PerformanceLevel, Status, SystemPowerState};

#[test]
fn wake_leases_block_suspend_until_removed() {
    let mut policy = PowerPolicy::new();

    assert_eq!(policy.add_lease(42, "download"), Status::Ok);
    assert_eq!(policy.active_lease_count(), 1);
    assert_eq!(
        policy.request_system_state(SystemPowerState::SuspendToRam),
        Status::ErrAccessDenied
    );

    policy.remove_lease(42);
    assert_eq!(policy.active_lease_count(), 0);
    assert_eq!(
        policy.request_system_state(SystemPowerState::SuspendToRam),
        Status::Ok
    );
}

#[test]
fn hibernate_is_deferred_and_device_registration_is_deduplicated() {
    let mut policy = PowerPolicy::new();

    assert_eq!(policy.register_device(7), Status::Ok);
    assert_eq!(policy.register_device(7), Status::Ok);
    assert_eq!(policy.device_endpoints(), &[7]);
    assert_eq!(
        policy.request_system_state(SystemPowerState::SuspendToDisk),
        Status::ErrUnsupported
    );
}

#[test]
fn provider_absence_reports_unavailable_without_fabricated_measurements() {
    let mut policy = PowerPolicy::new();
    let applied = policy.apply_telemetry_result(Err(Status::ErrUnsupported));
    let snapshot = applied.snapshot;
    assert!(!snapshot.battery_available);
    assert!(!snapshot.thermal_available);
    assert!(!snapshot.charging);
    assert_eq!(snapshot.battery_percent, 0);
    assert_eq!(snapshot.temperature_celsius, 0);
    assert!(!snapshot.telemetry_stale);
    assert_eq!(
        snapshot.applied_performance_level,
        PerformanceLevel::Nominal
    );
}

#[test]
fn thresholds_hysteresis_and_background_caps_select_safe_policy() {
    let mut policy = PowerPolicy::new();
    let reduced = policy.apply_telemetry_result(Ok(TelemetrySample {
        battery_available: true,
        charging: false,
        battery_percent: 15,
        thermal_available: true,
        temperature_celsius: 85,
    }));
    assert!(reduced.snapshot.battery_low);
    assert!(reduced.snapshot.thermal_throttled);
    assert_eq!(
        reduced.snapshot.applied_performance_level,
        PerformanceLevel::Reduced
    );
    assert_eq!(reduced.background_cpu_cap_permille, 500);

    let minimum = policy.apply_telemetry_result(Ok(TelemetrySample {
        battery_available: true,
        charging: true,
        battery_percent: 80,
        thermal_available: true,
        temperature_celsius: 95,
    }));
    assert_eq!(
        minimum.snapshot.applied_performance_level,
        PerformanceLevel::Minimum
    );
    assert_eq!(minimum.background_cpu_cap_permille, 250);

    let still_minimum = policy.apply_telemetry_result(Ok(TelemetrySample {
        thermal_available: true,
        temperature_celsius: 91,
        ..TelemetrySample::unavailable()
    }));
    assert_eq!(
        still_minimum.snapshot.applied_performance_level,
        PerformanceLevel::Minimum
    );

    let nominal = policy.apply_telemetry_result(Ok(TelemetrySample {
        thermal_available: true,
        temperature_celsius: 79,
        ..TelemetrySample::unavailable()
    }));
    assert_eq!(
        nominal.snapshot.applied_performance_level,
        PerformanceLevel::Nominal
    );
}

#[test]
fn stale_provider_failure_retains_throttle_until_recovery_sample() {
    let mut policy = PowerPolicy::new();
    assert_eq!(policy.register_telemetry_provider(77), Status::Ok);
    policy.apply_telemetry_result(Ok(TelemetrySample {
        thermal_available: true,
        temperature_celsius: 95,
        ..TelemetrySample::unavailable()
    }));
    let stale = policy.apply_telemetry_result(Err(Status::ErrIo)).snapshot;
    assert!(stale.telemetry_stale);
    assert_eq!(stale.applied_performance_level, PerformanceLevel::Minimum);

    let recovered = policy.apply_telemetry_result(Ok(TelemetrySample {
        thermal_available: true,
        temperature_celsius: 70,
        ..TelemetrySample::unavailable()
    }));
    assert!(!recovered.snapshot.telemetry_stale);
    assert_eq!(
        recovered.snapshot.applied_performance_level,
        PerformanceLevel::Nominal
    );
}

#[test]
fn runtime_migration_preserves_policy_and_handles() {
    let policy = PowerPolicy::from_parts(
        vec![WakeLease {
            handle: 11,
            reason: "display".into(),
        }],
        vec![21, 22],
        vec![SystemPowerState::Reboot],
    );
    let runtime = Runtime {
        control: Channel(1),
        migration: Some(Channel(2)),
        clients: vec![BoundServiceEndpoint::new(Channel(3), vec![1])],
        watchers: vec![4],
        policy,
        next_telemetry_poll_ms: 123,
    };
    let mut adopted = Runtime::empty();
    for key in runtime.keys() {
        let record = runtime.encode_record(key).unwrap();
        adopted.adopt_record(key, record.as_deref()).unwrap();
    }

    adopted.validate().unwrap();
    assert_eq!(
        adopted
            .clients
            .iter()
            .map(|client| client.channel.0)
            .collect::<Vec<_>>(),
        vec![3]
    );
    assert_eq!(adopted.policy.leases()[0].handle, 11);
    assert_eq!(adopted.policy.leases()[0].reason, "display");
    assert_eq!(adopted.policy.device_endpoints(), &[21, 22]);
    assert_eq!(adopted.policy.transitions, vec![SystemPowerState::Reboot]);
    assert_eq!(adopted.next_telemetry_poll_ms, 123);
    let handles = adopted
        .resources()
        .into_iter()
        .map(|resource| match resource {
            bexos_userspace::live_migration::Resource::Handle(handle) => handle,
            _ => 0,
        })
        .collect::<Vec<_>>();
    assert_eq!(handles, vec![1, 2, 3, 4, 21, 22, 11]);
}
