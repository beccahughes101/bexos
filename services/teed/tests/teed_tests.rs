use bexos_teed::{
    SoftwareEmuBackend, TeeService, TeeUpdateProgress, TrustedApp, derive_uuid, migration::Runtime,
};
use bexos_userspace::{Channel, live_migration::State};
use tee_manager_fidl::{
    FidlDecode, FidlEncode, HandleRef, TaState, TeeActivationMode, TeeKind,
    TeeManagerUpdateTeeCoreRequest, TeeStatus, TeeUpdatePhase, TeeUpdateStatus,
};

mod proxy_migration_tests;

#[test]
fn tee_manager_fixed_arrays_round_trip() {
    let state = TaState {
        uuid: [0x11; 16],
        version: 3,
        active_sessions: 2,
        entry_point_name: "software_emu_ta",
        storage_bytes_used: 4096,
        package_id: "bexos.ta.test",
        protected: true,
        package_managed: true,
    };
    let mut bytes = [0u8; 512];
    let mut handles = [];
    let encoded = state.encode(&mut bytes, &mut handles).unwrap();
    let decoded = TaState::decode(&bytes[..encoded.bytes], &[]).unwrap();
    assert_eq!(decoded.uuid, [0x11; 16]);

    let request = TeeManagerUpdateTeeCoreRequest {
        generation: 7,
        target: "qemu-aarch64-tee",
        activation: TeeActivationMode::LiveNow,
        artifact_hash: [0x42; 32],
        tee_image: HandleRef { raw: 9 },
        tee_image_len: 128,
    };
    let mut handles = [HandleRef { raw: 0 }; 1];
    let encoded = request.encode(&mut bytes, &mut handles).unwrap();
    let decoded = TeeManagerUpdateTeeCoreRequest::decode(
        &bytes[..encoded.bytes],
        &handles[..encoded.handles],
    )
    .unwrap();
    assert_eq!(decoded.artifact_hash, [0x42; 32]);
}

#[test]
fn software_backend_installs_lists_sessions_and_invokes() {
    test_runtime().block_on(async {
        let payload = b"trusted-app";
        let uuid = derive_uuid(payload);
        let mut service = TeeService::new(SoftwareEmuBackend::new());
        assert_eq!(service.info().await.unwrap().kind, TeeKind::SoftwareEmu);
        assert_eq!(service.install_app(payload).await, Ok(uuid));
        let session = service.open_session(uuid).await.unwrap();
        assert!(
            service
                .invoke(session, 7, b"hello")
                .await
                .unwrap()
                .bytes
                .ends_with(b"hello")
        );
        assert_eq!(
            service.uninstall_app(uuid).await,
            TeeStatus::ErrAccessDenied
        );
        assert_eq!(service.close_session(session).await, TeeStatus::Ok);
    });
}

#[test]
fn software_tee_update_tracks_completion_generation() {
    test_runtime().block_on(async {
        let mut service = TeeService::new(SoftwareEmuBackend::new());
        let hash = [0x42; 32];
        assert_eq!(
            service
                .update_core(
                    7,
                    "qemu-aarch64-tee",
                    TeeActivationMode::LiveNow,
                    &hash,
                    b"tee.bin",
                    0
                )
                .await,
            Ok(2)
        );
        let status = service.update_status().await.unwrap();
        assert_eq!(status.status, TeeUpdateStatus::Completed);
        assert_eq!(status.phase, TeeUpdatePhase::Completed);
        assert_eq!(status.generation, 7);
    });
}

#[test]
fn heart_transplant_preserves_typed_endpoint_sessions() {
    let uuid = [0x42; 16];
    let update = TeeUpdateProgress {
        status: TeeUpdateStatus::Completed,
        phase: TeeUpdatePhase::Completed,
        active_slot: "B".into(),
        pending_slot: String::new(),
        generation: 17,
        rollback_available: true,
        reboot_required: false,
        message: "active".into(),
    };
    let backend = SoftwareEmuBackend::from_parts(
        vec![TrustedApp {
            uuid,
            version: 4,
            active_sessions: 1,
            entry_point_name: "com.android.trusty.keymint".into(),
            storage_bytes_used: 4096,
            package_id: "bexos.ta.keymint".into(),
            service_ports: vec!["com.android.trusty.keymint".into()],
            protected: true,
            package_managed: false,
        }],
        vec![(23, uuid, Some("com.android.trusty.keymint".into()))],
        24,
        9,
        17,
        update,
    );
    let source = Runtime::new(Channel(1), Some(Channel(2)), TeeService::new(backend));
    let record = source.encode_record(0).unwrap().unwrap();
    let mut target = Runtime::<SoftwareEmuBackend>::empty();
    target.adopt_record(0, Some(&record)).unwrap();

    let (apps, sessions, next_session, secure_version, rollback_version, update) =
        target.service.backend().parts();
    assert_eq!(apps[0].package_id, "bexos.ta.keymint");
    assert_eq!(sessions[0].0, 23);
    assert_eq!(sessions[0].2.as_deref(), Some("com.android.trusty.keymint"));
    assert_eq!(next_session, 24);
    assert_eq!(secure_version, 9);
    assert_eq!(rollback_version, 17);
    assert_eq!(update.generation, 17);
}

fn test_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

#[test]
fn generic_tee_bindings_cannot_open_package_state_or_ambiguous_ports() {
    test_runtime().block_on(async {
        let mut service = TeeService::new(SoftwareEmuBackend::new());
        assert_eq!(
            service
                .open_endpoint("bexos.orchestrator", "com.bexos.package-state")
                .await,
            Err(TeeStatus::ErrAccessDenied)
        );
        for port in [
            "com.bexos.package-state\0suffix",
            "com.bexos.package-state\n",
            "com.bexos.package-state ",
        ] {
            assert_eq!(
                service.open_endpoint("bexos.orchestrator", port).await,
                Err(TeeStatus::ErrInvalidArgs)
            );
        }
    });
}
