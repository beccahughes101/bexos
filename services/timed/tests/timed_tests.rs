use bexos_timed::{nts, sntp, state};
use bexos_userspace::live_migration::State;
use bexos_userspace::service_binding::BoundServiceEndpoint;
use time_fidl::{ClockSource, Status, SyncState};

#[test]
fn sntp_request_uses_client_mode_v4_packet() {
    let mut packet = [0xff; 48];
    let len = sntp::encode_request(&mut packet).unwrap();

    assert_eq!(len, 48);
    assert_eq!(packet[0], 0x23);
    assert!(packet[1..].iter().all(|byte| *byte == 0));
}

#[test]
fn sntp_response_decodes_transmit_time_and_quality() {
    let mut packet = [0; 48];
    packet[0] = 0x24;
    packet[1] = 2;
    packet[8..12].copy_from_slice(&0x0001_8000u32.to_be_bytes());
    packet[40..44].copy_from_slice(&(2_208_988_800u32 + 10).to_be_bytes());
    packet[44..48].copy_from_slice(&0x8000_0000u32.to_be_bytes());

    let sample = sntp::decode_response(&packet).unwrap();
    let quality = sntp::quality_from_sample(sample, 4_000_000_000);

    assert_eq!(sample.unix_time_ns, 10_500_000_000);
    assert_eq!(quality.source, ClockSource::SntpNetwork);
    assert_eq!(quality.state, SyncState::Synced);
    assert_eq!(quality.stratum, 2);
    assert_eq!(quality.root_dispersion_ns, 1_500_000_000);
    assert_eq!(quality.utc_offset_ns, 6_500_000_000);
}

#[test]
fn nts_quality_marks_secure_source() {
    let sample = sntp::SntpSample {
        unix_time_ns: 10_500_000_000,
        stratum: 2,
        root_dispersion_ns: 1_500_000_000,
    };
    let quality = nts::quality_from_sample(sample, 4_000_000_000);
    assert_eq!(quality.source, ClockSource::NtsSecure);
    assert_eq!(quality.state, SyncState::Synced);
    assert_eq!(quality.utc_offset_ns, 6_500_000_000);
}

#[test]
fn nts_cookie_state_requires_keys_and_cookie() {
    let mut state = nts::NtsCookieState::default();
    assert!(!state.configured());
    state.c2s_key = vec![0x11; 32];
    state.s2c_key = vec![0x22; 32];
    state.cookies = vec![b"cookie".to_vec()];
    assert!(state.configured());
}

#[test]
fn state_round_trips_quality_record() {
    let quality = time_fidl::TimeQuality {
        source: ClockSource::ManualUser,
        state: SyncState::Manual,
        stratum: 0,
        root_dispersion_ns: 12,
        last_synced_timestamp_ns: 34,
        utc_offset_ns: -56,
        last_error: Status::Ok,
    };

    let bytes = state::encode(&quality);
    assert_eq!(state::decode(&bytes), Some(quality));
}

#[test]
fn guest_entry_is_tokio_future() {
    let _future = bexos_timed::main(0);
}

#[test]
fn migration_round_trips_quality_nts_clients_and_schedule() {
    let quality = time_fidl::TimeQuality {
        source: ClockSource::NtsSecure,
        state: SyncState::Synced,
        stratum: 2,
        root_dispersion_ns: 12,
        last_synced_timestamp_ns: 34,
        utc_offset_ns: -56,
        last_error: Status::Ok,
    };
    let nts_state = nts::NtsCookieState {
        c2s_key: vec![0x11; 32],
        s2c_key: vec![0x22; 32],
        cookies: vec![b"cookie".to_vec()],
        server: b"time.example".to_vec(),
        port: 123,
        replay_window: vec![vec![0x33; 32]],
    };
    let grants = vec![
        bexos_userspace::ServiceGrant {
            service: "bexos.net.Netstack".into(),
            protocol: "Netstack".into(),
            capability: "Public".into(),
            method_ordinals: vec![],
            permission_values: vec![],
            caller_package: None,
            caller_uid: None,
            caller_foreground: false,
            endpoint: 21,
        },
        bexos_userspace::ServiceGrant {
            service: "bexos.security.trust.TlsTrustManager".into(),
            protocol: "TlsTrustManager".into(),
            capability: "Public".into(),
            method_ordinals: vec![],
            permission_values: vec![],
            caller_package: None,
            caller_uid: None,
            caller_foreground: false,
            endpoint: 22,
        },
        bexos_userspace::ServiceGrant {
            service: "bexos.kernel.Clock".into(),
            protocol: "Clock".into(),
            capability: "Public".into(),
            method_ordinals: vec![],
            permission_values: vec![],
            caller_package: None,
            caller_uid: None,
            caller_foreground: false,
            endpoint: 0,
        },
        bexos_userspace::ServiceGrant {
            service: "bexos.kernel.SystemPrivileged".into(),
            protocol: "SystemPrivileged".into(),
            capability: "Public".into(),
            method_ordinals: vec![],
            permission_values: vec![],
            caller_package: None,
            caller_uid: None,
            caller_foreground: false,
            endpoint: 0,
        },
    ];
    let mut service = bexos_timed::service::TimedService::new(
        bexos_timed::config::TimedConfig::default(),
        bexos_userspace::Channel(0),
        grants,
    );
    service.set_migration_test_state(
        quality,
        nts_state.clone(),
        vec![BoundServiceEndpoint::new(
            bexos_userspace::Channel(23),
            vec![1, 2, 3, 4],
        )],
        vec![bexos_userspace::Channel(24)],
        99,
    );

    let runtime = bexos_timed::migration::Runtime::new(
        bexos_userspace::Channel(18),
        Some(bexos_userspace::Channel(19)),
        service,
    );
    let bytes = runtime.encode_record(0).unwrap().unwrap();
    let mut adopted = bexos_timed::migration::Runtime::empty();
    adopted.adopt_record(0, Some(&bytes)).unwrap();

    assert_eq!(adopted.control.0, 18);
    assert_eq!(adopted.migration.unwrap().0, 19);
    assert_eq!(adopted.service.quality(), quality);
    assert_eq!(adopted.service.nts_state(), &nts_state);
    assert_eq!(adopted.service.clients()[0].channel.0, 23);
    assert_eq!(adopted.service.quality_watchers()[0].0, 24);
    assert_eq!(
        adopted.service.clients()[0].allowed_methods,
        vec![1, 2, 3, 4]
    );
    assert_eq!(adopted.service.next_sync_ms(), 99);
}
