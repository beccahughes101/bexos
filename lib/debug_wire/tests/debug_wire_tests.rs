use bexos_debug_wire::{
    AppBundleChunkRequest, AppBundleUploadBeginRequest, AppBundleUploadCommitRequest, AppInfo,
    AppLaunchRequest, AppUninstallRequest, ComponentConfigGetRequest, ComponentConfigGetResponse,
    ComponentConfigMutationResponse, ComponentConfigResetRequest, ComponentConfigSetRequest,
    ExecRequest, ExecResponse, Frame, HealthCheckResponse, METHOD_HEALTH_CHECK, ProcessInfo,
    TraceStartRequest, TraceStatusResponse, TraceStopRequest, TraceStopResponse,
    UpdateCandidateInfo, UpdateCheckRequest, UpdateCheckResponse, UpdateChunkRequest,
    UpdateUploadBeginRequest, UpdateUploadCommitRequest, UserCreateRequest, UserInfo,
    UserListResponse, UserUnlockRequest, WireError, decode_app_bundle_chunk,
    decode_app_bundle_upload_begin, decode_app_bundle_upload_commit, decode_app_launch,
    decode_app_list, decode_app_uninstall, decode_component_config_get,
    decode_component_config_get_response, decode_component_config_mutation_response,
    decode_component_config_reset, decode_component_config_set, decode_exec_request,
    decode_exec_response, decode_health_response, decode_process_list, decode_trace_start_request,
    decode_trace_status_response, decode_trace_stop_request, decode_trace_stop_response,
    decode_update_check_request, decode_update_check_response, decode_update_chunk,
    decode_update_upload_begin, decode_update_upload_commit, decode_user_create_request,
    decode_user_list_response, decode_user_unlock_request, encode_app_bundle_chunk,
    encode_app_bundle_upload_begin, encode_app_bundle_upload_commit, encode_app_launch,
    encode_app_list, encode_app_uninstall, encode_component_config_get,
    encode_component_config_get_response, encode_component_config_mutation_response,
    encode_component_config_reset, encode_component_config_set, encode_exec_request,
    encode_exec_response, encode_health_response, encode_process_list, encode_trace_start_request,
    encode_trace_status_response, encode_trace_stop_request, encode_trace_stop_response,
    encode_update_check_request, encode_update_check_response, encode_update_chunk,
    encode_update_upload_begin, encode_update_upload_commit, encode_user_create_request,
    encode_user_list_response, encode_user_unlock_request, parse_frame,
};

#[test]
fn frame_round_trip_skips_serial_text() {
    let frame = Frame {
        flags: 0,
        request_id: 7,
        method_id: METHOD_HEALTH_CHECK,
        payload: b"abc".to_vec(),
    };
    let mut bytes = b"kernel: booting\n".to_vec();
    frame.encode(&mut bytes).unwrap();

    let (parsed, used) = parse_frame(&bytes).unwrap();
    assert_eq!(parsed, frame);
    assert_eq!(used, bytes.len());
}

#[test]
fn frame_reports_incomplete_and_rejects_bad_version() {
    assert_eq!(parse_frame(b"BXD1").unwrap_err(), WireError::Incomplete);

    let mut bytes = Vec::new();
    Frame {
        flags: 0,
        request_id: 1,
        method_id: 1,
        payload: Vec::new(),
    }
    .encode(&mut bytes)
    .unwrap();
    bytes[4] = 2;
    assert_eq!(
        parse_frame(&bytes).unwrap_err(),
        WireError::UnsupportedVersion
    );
}

#[test]
fn proto_round_trips_health_exec_and_processes() {
    let mut bytes = Vec::new();
    let health = HealthCheckResponse {
        service_name: "debugd".into(),
        status: "SERVING".into(),
        version: "qemu-socket-v1".into(),
    };
    encode_health_response(&health, &mut bytes);
    assert_eq!(decode_health_response(&bytes).unwrap(), health);

    let exec = ExecRequest {
        component_id: "debugd.version".into(),
        args: vec!["--short".into()],
    };
    encode_exec_request(&exec, &mut bytes);
    assert_eq!(decode_exec_request(&bytes).unwrap(), exec);

    let response = ExecResponse {
        exit_code: 0,
        stdout: "ok\n".into(),
        stderr: String::new(),
    };
    encode_exec_response(&response, &mut bytes);
    assert_eq!(decode_exec_response(&bytes).unwrap(), response);

    let processes = vec![ProcessInfo {
        pid: 1,
        name: "debugd".into(),
        state: "RUNNING".into(),
        package_id: "bexos.driver.debugd".into(),
        ..Default::default()
    }];
    encode_process_list(&processes, &mut bytes);
    assert_eq!(decode_process_list(&bytes).unwrap(), processes);
}

#[test]
fn proto_round_trips_app_lifecycle_messages() {
    let mut bytes = Vec::new();
    let apps = vec![AppInfo {
        package_id: "bexos.service.vfsd".into(),
        name: "vfsd".into(),
        state: "RUNNING".into(),
        source: "Bootfs".into(),
        protected: true,
    }];
    encode_app_list(&apps, &mut bytes);
    assert_eq!(decode_app_list(&bytes).unwrap(), apps);

    let begin = AppBundleUploadBeginRequest {
        upload_id: 99,
        archive_len: 4096,
    };
    encode_app_bundle_upload_begin(&begin, &mut bytes);
    assert_eq!(decode_app_bundle_upload_begin(&bytes).unwrap(), begin);

    let chunk = AppBundleChunkRequest {
        upload_id: 99,
        offset: 128,
        bytes: b"bundle".to_vec(),
    };
    encode_app_bundle_chunk(&chunk, &mut bytes);
    assert_eq!(decode_app_bundle_chunk(&bytes).unwrap(), chunk);

    let commit = AppBundleUploadCommitRequest { upload_id: 99 };
    encode_app_bundle_upload_commit(&commit, &mut bytes);
    assert_eq!(decode_app_bundle_upload_commit(&bytes).unwrap(), commit);

    let uninstall = AppUninstallRequest {
        package_id: "com.example:demo".into(),
    };
    encode_app_uninstall(&uninstall, &mut bytes);
    assert_eq!(decode_app_uninstall(&bytes).unwrap(), uninstall);

    let launch = AppLaunchRequest {
        package_id: "com.example:demo".into(),
        process_name: "demo".into(),
        arg0: 7,
        uid: 0,
    };
    encode_app_launch(&launch, &mut bytes);
    assert_eq!(decode_app_launch(&bytes).unwrap(), launch);
}

#[test]
fn proto_round_trips_component_config_messages() {
    let mut bytes = Vec::new();
    let get = ComponentConfigGetRequest {
        package_id: "bexos.service.netstackd".into(),
    };
    encode_component_config_get(&get, &mut bytes);
    assert_eq!(decode_component_config_get(&bytes).unwrap(), get);

    let get_response = ComponentConfigGetResponse {
        status: 0,
        generation: 4,
        config: b"BEXCFG".to_vec(),
    };
    encode_component_config_get_response(&get_response, &mut bytes);
    assert_eq!(
        decode_component_config_get_response(&bytes).unwrap(),
        get_response
    );

    let set = ComponentConfigSetRequest {
        package_id: "bexos.service.netstackd".into(),
        expected_generation: 4,
        config: b"BEXCFG".to_vec(),
    };
    encode_component_config_set(&set, &mut bytes);
    assert_eq!(decode_component_config_set(&bytes).unwrap(), set);

    let reset = ComponentConfigResetRequest {
        package_id: "bexos.service.netstackd".into(),
        expected_generation: 5,
    };
    encode_component_config_reset(&reset, &mut bytes);
    assert_eq!(decode_component_config_reset(&bytes).unwrap(), reset);

    let mutation = ComponentConfigMutationResponse {
        status: 0,
        generation: 5,
        message: "component config updated".into(),
    };
    encode_component_config_mutation_response(&mutation, &mut bytes);
    assert_eq!(
        decode_component_config_mutation_response(&bytes).unwrap(),
        mutation
    );
}

#[test]
fn proto_round_trips_update_upload_messages() {
    let mut bytes = Vec::new();
    let begin = UpdateUploadBeginRequest {
        upload_id: 101,
        manifest_len: 256,
        artifact_len: 4096,
    };
    encode_update_upload_begin(&begin, &mut bytes);
    assert_eq!(decode_update_upload_begin(&bytes).unwrap(), begin);

    let chunk = UpdateChunkRequest {
        upload_id: 101,
        stream: 2,
        offset: 128,
        bytes: b"artifact".to_vec(),
    };
    encode_update_chunk(&chunk, &mut bytes);
    assert_eq!(decode_update_chunk(&bytes).unwrap(), chunk);

    let commit = UpdateUploadCommitRequest { upload_id: 101 };
    encode_update_upload_commit(&commit, &mut bytes);
    assert_eq!(decode_update_upload_commit(&bytes).unwrap(), commit);
}

#[test]
fn proto_round_trips_update_check_messages() {
    let mut bytes = Vec::new();
    let request = UpdateCheckRequest {
        selector_kind: 2,
        target: "com.bexos.demo".into(),
        all: false,
        stage: true,
        apply: false,
    };
    encode_update_check_request(&request, &mut bytes);
    assert_eq!(decode_update_check_request(&bytes).unwrap(), request);

    let response = UpdateCheckResponse {
        status: 0,
        message: "updates available".into(),
        candidates: vec![UpdateCandidateInfo {
            target: "com.bexos.demo".into(),
            path: "apps/demo.bex".into(),
            url: "https://repo.example/apps/demo.bex".into(),
            kind: 1,
            generation: 4,
            length: 17,
        }],
    };
    encode_update_check_response(&response, &mut bytes);
    assert_eq!(decode_update_check_response(&bytes).unwrap(), response);
}

#[test]
fn proto_round_trips_trace_messages() {
    let mut bytes = Vec::new();
    let start = TraceStartRequest {
        categories: 0x42,
        buffer_mode: 2,
        buffer_size_kb: 2048,
        output_format: 1,
    };
    encode_trace_start_request(&start, &mut bytes);
    assert_eq!(decode_trace_start_request(&bytes).unwrap(), start);

    let status = TraceStatusResponse {
        status: 0,
        state: 2,
        categories: 0x42,
        buffer_mode: 2,
        buffer_size_kb: 2048,
        output_format: 1,
        producer_count: 3,
        event_count: 99,
        dropped_count: 1,
    };
    encode_trace_status_response(&status, &mut bytes);
    assert_eq!(decode_trace_status_response(&bytes).unwrap(), status);

    let stop_request = TraceStopRequest {
        offset: 4,
        max_bytes: 1024,
    };
    encode_trace_stop_request(&stop_request, &mut bytes);
    assert_eq!(decode_trace_stop_request(&bytes).unwrap(), stop_request);

    let stop_response = TraceStopResponse {
        status: 0,
        offset: 4,
        total_len: 8,
        bytes: b"trace".to_vec(),
        complete: true,
        output_format: 1,
    };
    encode_trace_stop_response(&stop_response, &mut bytes);
    assert_eq!(decode_trace_stop_response(&bytes).unwrap(), stop_response);
}

#[test]
fn proto_round_trips_user_management_messages() {
    let mut bytes = Vec::new();
    let create = UserCreateRequest {
        uid: 1000,
        name: "alice".into(),
        display_name: "Alice".into(),
        password: "password".into(),
    };
    encode_user_create_request(&create, &mut bytes);
    assert_eq!(decode_user_create_request(&bytes).unwrap(), create);

    let unlock = UserUnlockRequest {
        uid: 1000,
        password: "secret".into(),
    };
    encode_user_unlock_request(&unlock, &mut bytes);
    assert_eq!(decode_user_unlock_request(&bytes).unwrap(), unlock);

    let list = UserListResponse {
        status: 0,
        users: vec![UserInfo {
            uid: 1000,
            name: "alice".into(),
            display_name: "Alice".into(),
            disabled: false,
            home_path: "data/users/1000".into(),
            unlocked: true,
        }],
    };
    encode_user_list_response(&list, &mut bytes);
    assert_eq!(decode_user_list_response(&bytes).unwrap(), list);
}

#[test]
fn shell_wire_preserves_binary_data_and_rejects_oversized_requests() {
    use bexos_debug_wire::*;
    let q = ShellRequest {
        uid: u64::MAX,
        user: "alice".into(),
        password: "secret".into(),
        input: vec![0, 255, 3],
        rows: 42,
        cols: 123,
        ..Default::default()
    };
    let mut bytes = Vec::new();
    encode_shell_request(&q, &mut bytes);
    let r = decode_shell_request(&bytes).unwrap();
    assert_eq!(r.uid, u64::MAX);
    assert_eq!(r.input, q.input);
    assert_eq!(r.password, "secret");
    let q = ShellRequest {
        input: vec![0; SHELL_CHUNK + 1],
        ..Default::default()
    };
    encode_shell_request(&q, &mut bytes);
    assert!(matches!(
        decode_shell_request(&bytes),
        Err(WireError::PayloadTooLarge)
    ));
}
#[test]
fn process_metadata_is_optional_and_preserves_names() {
    use bexos_debug_wire::*;
    let mut old = Vec::new();
    let p = ProcessInfo {
        pid: 1,
        name: "a".into(),
        ..Default::default()
    };
    encode_process_list(&[p], &mut old);
    let r = decode_process_list(&old).unwrap();
    assert_eq!(r[0].resource_group_id, None);
    let p = ProcessInfo {
        pid: 2,
        main_thread_id: 3,
        resource_group_id: Some(7),
        resource_group_name: "render".into(),
        parent_resource_group_id: Some(1),
        parent_resource_group_name: "system".into(),
        ..Default::default()
    };
    encode_process_list(&[p.clone()], &mut old);
    assert_eq!(decode_process_list(&old).unwrap(), vec![p]);
}
#[test]
fn legacy_process_message_with_no_metadata_decodes() {
    use bexos_debug_wire::*;
    let bytes = [10, 5, 8, 1, 18, 1, b'a'];
    let p = decode_process_list(&bytes).unwrap();
    assert_eq!(p[0].pid, 1);
    assert_eq!(p[0].name, "a");
    assert_eq!(p[0].resource_group_id, None);
}
#[test]
fn firmware_status_preserves_reboot_and_recovery_outcomes() {
    use bexos_debug_wire::*;
    for phase in [
        "RebootPending",
        "Completed",
        "RolledBack",
        "RecoveryRequired",
    ] {
        let expected = TeeUpdateStatusResponse {
            status: 0,
            update_status: "Staged".into(),
            generation: 42,
            phase: phase.into(),
            active_slot: "A".into(),
            pending_slot: "B".into(),
            reboot_required: phase == "RebootPending",
            rollback_available: true,
            ..Default::default()
        };
        let mut bytes = Vec::new();
        encode_tee_update_status_response(&expected, &mut bytes);
        assert_eq!(decode_tee_update_status_response(&bytes).unwrap(), expected);
    }
    // A pre-extension peer sends status and generation without fields 5..9.
    let legacy = decode_tee_update_status_response(&[8, 0, 24, 7]).unwrap();
    assert_eq!(legacy.generation, 7);
    assert!(legacy.phase.is_empty());
    assert!(!legacy.reboot_required);
    assert!(!legacy.rollback_available);
}
#[test]
fn protobuf_rejects_overflow_and_skips_unknown_fixed_fields() {
    use bexos_debug_wire::*;
    let mut bytes = vec![8];
    bytes.extend_from_slice(&[255; 9]);
    bytes.push(2);
    assert!(decode_debug_status(&bytes).is_err());
    let bytes = [0x79, 0, 0, 0, 0, 0, 0, 0, 0, 8, 0];
    assert_eq!(decode_debug_status(&bytes).unwrap().status, 0);
    assert!(decode_debug_status(&[0, 0]).is_err());
}
