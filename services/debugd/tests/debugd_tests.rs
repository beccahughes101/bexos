mod processes_tests;

use bexos_debug_wire::{
    AppBundleChunkRequest, AppBundleUploadBeginRequest, AppBundleUploadCommitRequest,
    AppLaunchRequest, AppUninstallRequest, ExecRequest, Frame, METHOD_BEGIN_APP_BUNDLE_UPLOAD,
    METHOD_BEGIN_TEST_APP_UPLOAD, METHOD_BEGIN_UPDATE_UPLOAD, METHOD_CHECK_UPDATES,
    METHOD_COMMIT_APP_BUNDLE_UPLOAD, METHOD_COMMIT_TEST_APP_UPLOAD, METHOD_COMMIT_UPDATE_UPLOAD,
    METHOD_CREATE_USER, METHOD_EXEC_COMMAND, METHOD_HEALTH_CHECK, METHOD_LAUNCH_APP,
    METHOD_LAUNCH_TEST_APP, METHOD_LIST_APPS, METHOD_LIST_PROCESSES, METHOD_LIST_USERS,
    METHOD_LOCK_USER, METHOD_TEE_CLOSE_SESSION, METHOD_TEE_INFO, METHOD_TEE_INSTALL_APP,
    METHOD_TEE_INVOKE, METHOD_TEE_LIST_APPS, METHOD_TEE_OPEN_SESSION, METHOD_TEE_UNINSTALL_APP,
    METHOD_TEE_UPDATE_STATUS, METHOD_TRACE_START, METHOD_TRACE_STATUS, METHOD_TRACE_STOP,
    METHOD_UNINSTALL_APP, METHOD_UNLOCK_USER, METHOD_WRITE_APP_BUNDLE_CHUNK,
    METHOD_WRITE_TEST_APP_CHUNK, METHOD_WRITE_UPDATE_CHUNK, ProcessInfo, TeeInstallAppRequest,
    TeeInvokeRequest, TeeSessionRequest, TeeUuidRequest, TestAppChunkRequest, TestAppLaunchRequest,
    TestAppUploadBeginRequest, TestAppUploadCommitRequest, TraceStartRequest, TraceStopRequest,
    UpdateCandidateInfo, UpdateCheckRequest, UpdateChunkRequest, UpdateUploadBeginRequest,
    UpdateUploadCommitRequest, UserCreateRequest, UserLockRequest, UserUnlockRequest,
    decode_app_list, decode_debug_status, decode_exec_response, decode_health_response,
    decode_process_list, decode_tee_app_list_response, decode_tee_info_response,
    decode_tee_install_app_response, decode_tee_invoke_response, decode_tee_open_session_response,
    decode_tee_update_status_response, decode_trace_status_response, decode_trace_stop_response,
    decode_update_check_response, decode_user_list_response, encode_app_bundle_chunk,
    encode_app_bundle_upload_begin, encode_app_bundle_upload_commit, encode_app_launch,
    encode_app_uninstall, encode_exec_request, encode_tee_install_app_request,
    encode_tee_invoke_request, encode_tee_session_request, encode_tee_uuid_request,
    encode_test_app_chunk, encode_test_app_launch, encode_test_app_upload_begin,
    encode_test_app_upload_commit, encode_trace_start_request, encode_trace_stop_request,
    encode_update_check_request, encode_update_chunk, encode_update_upload_begin,
    encode_update_upload_commit, encode_user_create_request, encode_user_lock_request,
    encode_user_unlock_request,
};
use bexos_debugd::{
    BufferedAppManager, BufferedTeeManager, BufferedTestAppInstaller, BufferedTraceManager,
    BufferedUpdateManager, BufferedUserManager, PlatformUpdateApplier, TeeManager, handle_frame,
    handle_frame_with_all_backends, handle_frame_with_backends, handle_frame_with_installer,
    handle_frame_with_trace_backends,
};
use bexos_update::{ArtifactKind, UpdateManifest, encode_unsigned_manifest};
use ed25519_dalek::{Signer, SigningKey};

fn block_on<T>(future: impl core::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("debugd test tokio runtime")
        .block_on(future)
}

const KEY_ID: [u8; 32] = *b"bexos-qemu-test-ed25519-key-v001";
const SEED: [u8; 32] = [
    0x9d, 0x61, 0xb1, 0x9d, 0xef, 0xfd, 0x5a, 0x60, 0xba, 0x84, 0x4a, 0xf4, 0x92, 0xec, 0x2c, 0xc4,
    0x44, 0x49, 0xc5, 0x69, 0x7b, 0x32, 0x69, 0x19, 0x70, 0x3b, 0xac, 0x03, 0x1c, 0xae, 0x7f, 0x60,
];

fn signed_manifest(generation: u64, target: &str, kind: ArtifactKind, artifact: &[u8]) -> Vec<u8> {
    let mut manifest = encode_unsigned_manifest(generation, target, kind, artifact, KEY_ID);
    let signature = SigningKey::from_bytes(&SEED).sign(&manifest).to_bytes();
    manifest[104..168].copy_from_slice(&signature);
    manifest
}

fn processes() -> Vec<ProcessInfo> {
    vec![ProcessInfo {
        pid: 42,
        name: "debugd".into(),
        state: "RUNNING".into(),
        package_id: "bexos.driver.debugd".into(),
        ..Default::default()
    }]
}

#[derive(Default)]
struct FakePlatformUpdate {
    applied: bool,
}

impl PlatformUpdateApplier for FakePlatformUpdate {
    async fn apply_platform_update(
        &mut self,
        _manifest: &UpdateManifest,
        _artifact: &[u8],
    ) -> bexos_debug_wire::DebugStatusResponse {
        self.applied = true;
        bexos_debug_wire::DebugStatusResponse {
            status: 0,
            message: "fake platform update applied".into(),
        }
    }

    async fn platform_update_status(&mut self) -> bexos_debug_wire::DebugStatusResponse {
        bexos_debug_wire::DebugStatusResponse {
            status: 0,
            message: if self.applied {
                "fake platform completed".into()
            } else {
                "fake platform idle".into()
            },
        }
    }
}

#[test]
fn health_check_reports_serving() {
    let response = block_on(handle_frame(
        &Frame {
            flags: 0,
            request_id: 9,
            method_id: METHOD_HEALTH_CHECK,
            payload: Vec::new(),
        },
        &processes(),
    ));
    assert_eq!(response.request_id, 9);
    let health = decode_health_response(&response.payload).unwrap();
    assert_eq!(health.service_name, "debugd");
    assert_eq!(health.status, "SERVING");
}

#[test]
fn debugd_frame_handler_is_tokio_future() {
    let request = Frame {
        flags: 0,
        request_id: 1,
        method_id: METHOD_HEALTH_CHECK,
        payload: Vec::new(),
    };
    let processes = processes();
    let _future = handle_frame(&request, &processes);
}

#[test]
fn trace_methods_start_status_and_stop() {
    let mut installer = BufferedTestAppInstaller::new();
    let mut apps = BufferedAppManager::new();
    let mut updates = BufferedUpdateManager::new();
    let mut platform = FakePlatformUpdate::default();
    let mut users = BufferedUserManager::new();
    let mut tee = BufferedTeeManager::new();
    let mut traces = BufferedTraceManager::new();

    let mut payload = Vec::new();
    encode_trace_start_request(
        &TraceStartRequest {
            categories: bexos_trace::CATEGORY_DEBUG_SERVICE,
            buffer_mode: bexos_trace::BufferMode::CircularRing.to_wire(),
            buffer_size_kb: 64,
            output_format: bexos_trace::TraceOutputFormat::Perfetto.to_wire(),
        },
        &mut payload,
    );
    let response = block_on(handle_frame_with_trace_backends(
        &Frame {
            flags: 0,
            request_id: 10,
            method_id: METHOD_TRACE_START,
            payload,
        },
        &processes(),
        &mut installer,
        &mut apps,
        &mut updates,
        &mut platform,
        &mut users,
        &mut tee,
        &mut traces,
    ));
    assert_eq!(decode_debug_status(&response.payload).unwrap().status, 0);

    let response = block_on(handle_frame_with_trace_backends(
        &Frame {
            flags: 0,
            request_id: 11,
            method_id: METHOD_TRACE_STATUS,
            payload: Vec::new(),
        },
        &processes(),
        &mut installer,
        &mut apps,
        &mut updates,
        &mut platform,
        &mut users,
        &mut tee,
        &mut traces,
    ));
    let status = decode_trace_status_response(&response.payload).unwrap();
    assert_eq!(status.status, 0);
    assert_eq!(status.state, 2);
    assert!(status.event_count >= 2);

    let mut payload = Vec::new();
    encode_trace_stop_request(
        &TraceStopRequest {
            offset: 0,
            max_bytes: 48 * 1024,
        },
        &mut payload,
    );
    let response = block_on(handle_frame_with_trace_backends(
        &Frame {
            flags: 0,
            request_id: 12,
            method_id: METHOD_TRACE_STOP,
            payload,
        },
        &processes(),
        &mut installer,
        &mut apps,
        &mut updates,
        &mut platform,
        &mut users,
        &mut tee,
        &mut traces,
    ));
    let trace = decode_trace_stop_response(&response.payload).unwrap();
    assert_eq!(trace.status, 0);
    assert!(trace.complete);
    assert!(bexos_trace::looks_like_perfetto_trace(&trace.bytes));
    assert!(
        trace
            .bytes
            .windows(b"debugd:trace_start".len())
            .any(|w| w == b"debugd:trace_start")
    );
}

#[test]
fn list_processes_returns_supplied_snapshot() {
    let response = block_on(handle_frame(
        &Frame {
            flags: 0,
            request_id: 1,
            method_id: METHOD_LIST_PROCESSES,
            payload: Vec::new(),
        },
        &processes(),
    ));
    let decoded = decode_process_list(&response.payload).unwrap();
    assert_eq!(decoded[0].pid, 42);
}

#[test]
fn exec_supports_only_diagnostic_commands() {
    let mut payload = Vec::new();
    encode_exec_request(
        &ExecRequest {
            component_id: "debugd.version".into(),
            args: Vec::new(),
        },
        &mut payload,
    );
    let response = block_on(handle_frame(
        &Frame {
            flags: 0,
            request_id: 2,
            method_id: METHOD_EXEC_COMMAND,
            payload,
        },
        &processes(),
    ));
    assert_eq!(
        decode_exec_response(&response.payload).unwrap().exit_code,
        0
    );

    let mut payload = Vec::new();
    encode_exec_request(
        &ExecRequest {
            component_id: "sh".into(),
            args: vec!["-c".into(), "echo nope".into()],
        },
        &mut payload,
    );
    let response = block_on(handle_frame(
        &Frame {
            flags: 0,
            request_id: 3,
            method_id: METHOD_EXEC_COMMAND,
            payload,
        },
        &processes(),
    ));
    assert_eq!(
        decode_exec_response(&response.payload).unwrap().exit_code,
        127
    );
}

#[test]
fn test_app_upload_buffers_chunks_and_rejects_launch_without_appd_backend() {
    let mut installer = BufferedTestAppInstaller::new();
    let mut payload = Vec::new();
    encode_test_app_upload_begin(
        &TestAppUploadBeginRequest {
            upload_id: 7,
            package_id: "bexos.platform.storage_verify".into(),
            manifest_len: 4,
            elf_len: 3,
        },
        &mut payload,
    );
    let response = block_on(handle_frame_with_installer(
        &Frame {
            flags: 0,
            request_id: 4,
            method_id: METHOD_BEGIN_TEST_APP_UPLOAD,
            payload,
        },
        &processes(),
        &mut installer,
    ));
    assert_eq!(decode_debug_status(&response.payload).unwrap().status, 0);

    let mut payload = Vec::new();
    encode_test_app_chunk(
        &TestAppChunkRequest {
            upload_id: 7,
            stream: 1,
            offset: 0,
            bytes: b"mani".to_vec(),
        },
        &mut payload,
    );
    let response = block_on(handle_frame_with_installer(
        &Frame {
            flags: 0,
            request_id: 5,
            method_id: METHOD_WRITE_TEST_APP_CHUNK,
            payload,
        },
        &processes(),
        &mut installer,
    ));
    assert_eq!(decode_debug_status(&response.payload).unwrap().status, 0);

    let mut payload = Vec::new();
    encode_test_app_chunk(
        &TestAppChunkRequest {
            upload_id: 7,
            stream: 2,
            offset: 0,
            bytes: b"elf".to_vec(),
        },
        &mut payload,
    );
    let response = block_on(handle_frame_with_installer(
        &Frame {
            flags: 0,
            request_id: 6,
            method_id: METHOD_WRITE_TEST_APP_CHUNK,
            payload,
        },
        &processes(),
        &mut installer,
    ));
    assert_eq!(decode_debug_status(&response.payload).unwrap().status, 0);

    let mut payload = Vec::new();
    encode_test_app_upload_commit(&TestAppUploadCommitRequest { upload_id: 7 }, &mut payload);
    let response = block_on(handle_frame_with_installer(
        &Frame {
            flags: 0,
            request_id: 7,
            method_id: METHOD_COMMIT_TEST_APP_UPLOAD,
            payload,
        },
        &processes(),
        &mut installer,
    ));
    assert_eq!(decode_debug_status(&response.payload).unwrap().status, 0);

    let mut payload = Vec::new();
    encode_test_app_launch(
        &TestAppLaunchRequest {
            package_id: "bexos.platform.storage_verify".into(),
            process_name: "storage_verify".into(),
            arg0: 1,
        },
        &mut payload,
    );
    let response = block_on(handle_frame_with_installer(
        &Frame {
            flags: 0,
            request_id: 8,
            method_id: METHOD_LAUNCH_TEST_APP,
            payload,
        },
        &processes(),
        &mut installer,
    ));
    assert_ne!(decode_debug_status(&response.payload).unwrap().status, 0);
}

#[test]
fn app_bundle_methods_route_to_app_manager_backend() {
    let mut installer = BufferedTestAppInstaller::new();
    let mut apps = BufferedAppManager::new();
    let mut updates = BufferedUpdateManager::new();
    let mut platform = FakePlatformUpdate::default();
    apps.seed_app("bexos.service.vfsd", "vfsd");

    let response = block_on(handle_frame_with_backends(
        &Frame {
            flags: 0,
            request_id: 9,
            method_id: METHOD_LIST_APPS,
            payload: Vec::new(),
        },
        &processes(),
        &mut installer,
        &mut apps,
        &mut updates,
        &mut platform,
    ));
    let listed = decode_app_list(&response.payload).unwrap();
    assert_eq!(listed[0].package_id, "bexos.service.vfsd");

    let mut payload = Vec::new();
    encode_app_bundle_upload_begin(
        &AppBundleUploadBeginRequest {
            upload_id: 11,
            archive_len: 6,
        },
        &mut payload,
    );
    let response = block_on(handle_frame_with_backends(
        &Frame {
            flags: 0,
            request_id: 10,
            method_id: METHOD_BEGIN_APP_BUNDLE_UPLOAD,
            payload,
        },
        &processes(),
        &mut installer,
        &mut apps,
        &mut updates,
        &mut platform,
    ));
    assert_eq!(decode_debug_status(&response.payload).unwrap().status, 0);

    let mut payload = Vec::new();
    encode_app_bundle_chunk(
        &AppBundleChunkRequest {
            upload_id: 11,
            offset: 0,
            bytes: b"bundle".to_vec(),
        },
        &mut payload,
    );
    let response = block_on(handle_frame_with_backends(
        &Frame {
            flags: 0,
            request_id: 11,
            method_id: METHOD_WRITE_APP_BUNDLE_CHUNK,
            payload,
        },
        &processes(),
        &mut installer,
        &mut apps,
        &mut updates,
        &mut platform,
    ));
    assert_eq!(decode_debug_status(&response.payload).unwrap().status, 0);

    let mut payload = Vec::new();
    encode_app_bundle_upload_commit(
        &AppBundleUploadCommitRequest { upload_id: 11 },
        &mut payload,
    );
    let response = block_on(handle_frame_with_backends(
        &Frame {
            flags: 0,
            request_id: 12,
            method_id: METHOD_COMMIT_APP_BUNDLE_UPLOAD,
            payload,
        },
        &processes(),
        &mut installer,
        &mut apps,
        &mut updates,
        &mut platform,
    ));
    assert_eq!(decode_debug_status(&response.payload).unwrap().status, 0);

    let mut payload = Vec::new();
    encode_app_launch(
        &AppLaunchRequest {
            package_id: "debug.bundle.2".into(),
            process_name: "main".into(),
            arg0: 0,
            uid: 0,
        },
        &mut payload,
    );
    let response = block_on(handle_frame_with_backends(
        &Frame {
            flags: 0,
            request_id: 13,
            method_id: METHOD_LAUNCH_APP,
            payload,
        },
        &processes(),
        &mut installer,
        &mut apps,
        &mut updates,
        &mut platform,
    ));
    assert_eq!(decode_debug_status(&response.payload).unwrap().status, 0);

    let mut payload = Vec::new();
    encode_app_uninstall(
        &AppUninstallRequest {
            package_id: "debug.bundle.2".into(),
        },
        &mut payload,
    );
    let response = block_on(handle_frame_with_backends(
        &Frame {
            flags: 0,
            request_id: 14,
            method_id: METHOD_UNINSTALL_APP,
            payload,
        },
        &processes(),
        &mut installer,
        &mut apps,
        &mut updates,
        &mut platform,
    ));
    assert_eq!(decode_debug_status(&response.payload).unwrap().status, 0);
}

#[test]
fn user_methods_route_to_user_manager_backend() {
    let mut installer = BufferedTestAppInstaller::new();
    let mut apps = BufferedAppManager::new();
    let mut updates = BufferedUpdateManager::new();
    let mut platform = FakePlatformUpdate::default();
    let mut users = BufferedUserManager::new();
    let mut tee = BufferedTeeManager::new();

    let mut payload = Vec::new();
    encode_user_create_request(
        &UserCreateRequest {
            uid: 1000,
            name: "alice".into(),
            display_name: "Alice".into(),
            password: "password".into(),
        },
        &mut payload,
    );
    let response = block_on(handle_frame_with_all_backends(
        &Frame {
            flags: 0,
            request_id: 70,
            method_id: METHOD_CREATE_USER,
            payload,
        },
        &processes(),
        &mut installer,
        &mut apps,
        &mut updates,
        &mut platform,
        &mut users,
        &mut tee,
    ));
    assert_eq!(decode_debug_status(&response.payload).unwrap().status, 0);

    let response = block_on(handle_frame_with_all_backends(
        &Frame {
            flags: 0,
            request_id: 71,
            method_id: METHOD_LIST_USERS,
            payload: Vec::new(),
        },
        &processes(),
        &mut installer,
        &mut apps,
        &mut updates,
        &mut platform,
        &mut users,
        &mut tee,
    ));
    let listed = decode_user_list_response(&response.payload).unwrap();
    assert_eq!(listed.users[0].uid, 1000);

    let mut payload = Vec::new();
    encode_user_unlock_request(
        &UserUnlockRequest {
            uid: 1000,
            password: "pw".into(),
        },
        &mut payload,
    );
    let response = block_on(handle_frame_with_all_backends(
        &Frame {
            flags: 0,
            request_id: 72,
            method_id: METHOD_UNLOCK_USER,
            payload,
        },
        &processes(),
        &mut installer,
        &mut apps,
        &mut updates,
        &mut platform,
        &mut users,
        &mut tee,
    ));
    assert_eq!(decode_debug_status(&response.payload).unwrap().status, 0);

    let mut payload = Vec::new();
    encode_user_lock_request(&UserLockRequest { uid: 1000 }, &mut payload);
    let response = block_on(handle_frame_with_all_backends(
        &Frame {
            flags: 0,
            request_id: 73,
            method_id: METHOD_LOCK_USER,
            payload,
        },
        &processes(),
        &mut installer,
        &mut apps,
        &mut updates,
        &mut platform,
        &mut users,
        &mut tee,
    ));
    assert_eq!(decode_debug_status(&response.payload).unwrap().status, 0);
}

#[test]
fn update_methods_stage_bytes_and_unknown_exec_still_fails() {
    let mut installer = BufferedTestAppInstaller::new();
    let mut apps = BufferedAppManager::new();
    let mut updates = BufferedUpdateManager::new();
    let mut platform = FakePlatformUpdate::default();

    let mut payload = Vec::new();
    encode_update_upload_begin(
        &UpdateUploadBeginRequest {
            upload_id: 77,
            manifest_len: 4,
            artifact_len: 6,
        },
        &mut payload,
    );
    let response = block_on(handle_frame_with_backends(
        &Frame {
            flags: 0,
            request_id: 30,
            method_id: METHOD_BEGIN_UPDATE_UPLOAD,
            payload,
        },
        &processes(),
        &mut installer,
        &mut apps,
        &mut updates,
        &mut platform,
    ));
    assert_eq!(decode_debug_status(&response.payload).unwrap().status, 0);

    let mut payload = Vec::new();
    encode_update_chunk(
        &UpdateChunkRequest {
            upload_id: 77,
            stream: 1,
            offset: 0,
            bytes: b"mani".to_vec(),
        },
        &mut payload,
    );
    let response = block_on(handle_frame_with_backends(
        &Frame {
            flags: 0,
            request_id: 31,
            method_id: METHOD_WRITE_UPDATE_CHUNK,
            payload,
        },
        &processes(),
        &mut installer,
        &mut apps,
        &mut updates,
        &mut platform,
    ));
    assert_eq!(decode_debug_status(&response.payload).unwrap().status, 0);

    let mut payload = Vec::new();
    encode_update_chunk(
        &UpdateChunkRequest {
            upload_id: 77,
            stream: 2,
            offset: 0,
            bytes: b"kernel".to_vec(),
        },
        &mut payload,
    );
    let response = block_on(handle_frame_with_backends(
        &Frame {
            flags: 0,
            request_id: 32,
            method_id: METHOD_WRITE_UPDATE_CHUNK,
            payload,
        },
        &processes(),
        &mut installer,
        &mut apps,
        &mut updates,
        &mut platform,
    ));
    assert_eq!(decode_debug_status(&response.payload).unwrap().status, 0);

    let mut payload = Vec::new();
    encode_update_upload_commit(&UpdateUploadCommitRequest { upload_id: 77 }, &mut payload);
    let response = block_on(handle_frame_with_backends(
        &Frame {
            flags: 0,
            request_id: 33,
            method_id: METHOD_COMMIT_UPDATE_UPLOAD,
            payload,
        },
        &processes(),
        &mut installer,
        &mut apps,
        &mut updates,
        &mut platform,
    ));
    assert_eq!(decode_debug_status(&response.payload).unwrap().status, 0);

    let mut payload = Vec::new();
    encode_exec_request(
        &ExecRequest {
            component_id: "sh".into(),
            args: vec!["-c".into(), "echo nope".into()],
        },
        &mut payload,
    );
    let response = block_on(handle_frame_with_backends(
        &Frame {
            flags: 0,
            request_id: 34,
            method_id: METHOD_EXEC_COMMAND,
            payload,
        },
        &processes(),
        &mut installer,
        &mut apps,
        &mut updates,
        &mut platform,
    ));
    assert_eq!(
        decode_exec_response(&response.payload).unwrap().exit_code,
        127
    );
}

#[test]
fn update_check_method_lists_seeded_feed_candidate() {
    let mut installer = BufferedTestAppInstaller::new();
    let mut apps = BufferedAppManager::new();
    let mut updates = BufferedUpdateManager::new();
    updates.seed_feed_candidate(UpdateCandidateInfo {
        target: "com.bexos.demo".into(),
        path: "apps/demo.bex".into(),
        url: "https://repo.example/apps/demo.bex".into(),
        kind: 1,
        generation: 4,
        length: 17,
    });
    let mut platform = FakePlatformUpdate::default();

    let mut payload = Vec::new();
    encode_update_check_request(
        &UpdateCheckRequest {
            selector_kind: 2,
            target: "com.bexos.demo".into(),
            all: false,
            stage: false,
            apply: false,
        },
        &mut payload,
    );
    let response = block_on(handle_frame_with_backends(
        &Frame {
            flags: 0,
            request_id: 34,
            method_id: METHOD_CHECK_UPDATES,
            payload,
        },
        &processes(),
        &mut installer,
        &mut apps,
        &mut updates,
        &mut platform,
    ));
    let response = decode_update_check_response(&response.payload).unwrap();
    assert_eq!(response.status, 0);
    assert_eq!(response.candidates.len(), 1);
    assert_eq!(response.candidates[0].target, "com.bexos.demo");
}

#[test]
fn tee_image_update_routes_to_tee_manager_not_kernel_platform() {
    use bexos_debugd::service::UpdateManager;

    let artifact = b"tee core bytes";
    let manifest = signed_manifest(12, "qemu-aarch64-tee", ArtifactKind::TeeImage, artifact);
    let mut updates = BufferedUpdateManager::new();
    let mut platform = FakePlatformUpdate::default();
    let mut tee = BufferedTeeManager::new();

    assert_eq!(
        block_on(updates.begin_upload(UpdateUploadBeginRequest {
            upload_id: 12,
            manifest_len: manifest.len() as u64,
            artifact_len: artifact.len() as u64,
        }))
        .status,
        0
    );
    assert_eq!(
        block_on(updates.write_chunk(UpdateChunkRequest {
            upload_id: 12,
            stream: 1,
            offset: 0,
            bytes: manifest,
        }))
        .status,
        0
    );
    assert_eq!(
        block_on(updates.write_chunk(UpdateChunkRequest {
            upload_id: 12,
            stream: 2,
            offset: 0,
            bytes: artifact.to_vec(),
        }))
        .status,
        0
    );
    assert_eq!(
        block_on(updates.commit_upload(UpdateUploadCommitRequest { upload_id: 12 })).status,
        0
    );
    let applied = block_on(updates.apply_platform(&mut platform, &mut tee));
    assert_eq!(applied.status, 0);
    assert!(!platform.applied);
    assert_eq!(block_on(tee.update_status()).generation, 12);

    let rollback_manifest =
        signed_manifest(12, "qemu-aarch64-tee", ArtifactKind::TeeImage, artifact);
    assert_eq!(
        block_on(updates.begin_upload(UpdateUploadBeginRequest {
            upload_id: 13,
            manifest_len: rollback_manifest.len() as u64,
            artifact_len: artifact.len() as u64,
        }))
        .status,
        0
    );
    assert_eq!(
        block_on(updates.write_chunk(UpdateChunkRequest {
            upload_id: 13,
            stream: 1,
            offset: 0,
            bytes: rollback_manifest,
        }))
        .status,
        0
    );
    assert_eq!(
        block_on(updates.write_chunk(UpdateChunkRequest {
            upload_id: 13,
            stream: 2,
            offset: 0,
            bytes: artifact.to_vec(),
        }))
        .status,
        0
    );
    assert_eq!(
        block_on(updates.commit_upload(UpdateUploadCommitRequest { upload_id: 13 })).status,
        0
    );
    let rollback = block_on(updates.apply_platform(&mut platform, &mut tee));
    assert_ne!(rollback.status, 0);
    assert!(rollback.message.contains("Rollback"));
}

#[test]
fn tee_methods_route_to_tee_manager_backend() {
    let mut installer = BufferedTestAppInstaller::new();
    let mut apps = BufferedAppManager::new();
    let mut updates = BufferedUpdateManager::new();
    let mut platform = FakePlatformUpdate::default();
    let mut users = BufferedUserManager::new();
    let mut tee = BufferedTeeManager::new();

    let response = block_on(handle_frame_with_all_backends(
        &Frame {
            flags: 0,
            request_id: 80,
            method_id: METHOD_TEE_INFO,
            payload: Vec::new(),
        },
        &processes(),
        &mut installer,
        &mut apps,
        &mut updates,
        &mut platform,
        &mut users,
        &mut tee,
    ));
    let info = decode_tee_info_response(&response.payload).unwrap();
    assert_eq!(info.status, 0);
    assert_eq!(info.kind, "SoftwareEmu");

    let mut payload = Vec::new();
    encode_tee_install_app_request(
        &TeeInstallAppRequest {
            payload: b"trusted app".to_vec(),
        },
        &mut payload,
    );
    let response = block_on(handle_frame_with_all_backends(
        &Frame {
            flags: 0,
            request_id: 81,
            method_id: METHOD_TEE_INSTALL_APP,
            payload,
        },
        &processes(),
        &mut installer,
        &mut apps,
        &mut updates,
        &mut platform,
        &mut users,
        &mut tee,
    ));
    let installed = decode_tee_install_app_response(&response.payload).unwrap();
    assert_eq!(installed.status, 0);
    assert_eq!(installed.uuid.len(), 16);

    let response = block_on(handle_frame_with_all_backends(
        &Frame {
            flags: 0,
            request_id: 82,
            method_id: METHOD_TEE_LIST_APPS,
            payload: Vec::new(),
        },
        &processes(),
        &mut installer,
        &mut apps,
        &mut updates,
        &mut platform,
        &mut users,
        &mut tee,
    ));
    let list = decode_tee_app_list_response(&response.payload).unwrap();
    assert_eq!(list.apps.len(), 1);

    let mut payload = Vec::new();
    encode_tee_uuid_request(
        &TeeUuidRequest {
            uuid: installed.uuid.clone(),
        },
        &mut payload,
    );
    let response = block_on(handle_frame_with_all_backends(
        &Frame {
            flags: 0,
            request_id: 83,
            method_id: METHOD_TEE_OPEN_SESSION,
            payload,
        },
        &processes(),
        &mut installer,
        &mut apps,
        &mut updates,
        &mut platform,
        &mut users,
        &mut tee,
    ));
    let opened = decode_tee_open_session_response(&response.payload).unwrap();
    assert_eq!(opened.status, 0);

    let mut payload = Vec::new();
    encode_tee_invoke_request(
        &TeeInvokeRequest {
            session_id: opened.session_id,
            command_id: 9,
            payload: b"ping".to_vec(),
        },
        &mut payload,
    );
    let response = block_on(handle_frame_with_all_backends(
        &Frame {
            flags: 0,
            request_id: 84,
            method_id: METHOD_TEE_INVOKE,
            payload,
        },
        &processes(),
        &mut installer,
        &mut apps,
        &mut updates,
        &mut platform,
        &mut users,
        &mut tee,
    ));
    let invoked = decode_tee_invoke_response(&response.payload).unwrap();
    assert_eq!(invoked.status, 0);
    assert!(invoked.response.ends_with(b"ping"));

    let mut payload = Vec::new();
    encode_tee_session_request(
        &TeeSessionRequest {
            session_id: opened.session_id,
        },
        &mut payload,
    );
    let response = block_on(handle_frame_with_all_backends(
        &Frame {
            flags: 0,
            request_id: 85,
            method_id: METHOD_TEE_CLOSE_SESSION,
            payload,
        },
        &processes(),
        &mut installer,
        &mut apps,
        &mut updates,
        &mut platform,
        &mut users,
        &mut tee,
    ));
    assert_eq!(decode_debug_status(&response.payload).unwrap().status, 0);

    let mut payload = Vec::new();
    encode_tee_uuid_request(
        &TeeUuidRequest {
            uuid: installed.uuid,
        },
        &mut payload,
    );
    let response = block_on(handle_frame_with_all_backends(
        &Frame {
            flags: 0,
            request_id: 86,
            method_id: METHOD_TEE_UNINSTALL_APP,
            payload,
        },
        &processes(),
        &mut installer,
        &mut apps,
        &mut updates,
        &mut platform,
        &mut users,
        &mut tee,
    ));
    assert_eq!(decode_debug_status(&response.payload).unwrap().status, 0);

    let response = block_on(handle_frame_with_all_backends(
        &Frame {
            flags: 0,
            request_id: 87,
            method_id: METHOD_TEE_UPDATE_STATUS,
            payload: Vec::new(),
        },
        &processes(),
        &mut installer,
        &mut apps,
        &mut updates,
        &mut platform,
        &mut users,
        &mut tee,
    ));
    assert_eq!(
        decode_tee_update_status_response(&response.payload)
            .unwrap()
            .update_status,
        "Idle"
    );
}

#[test]
fn update_checkpoint_retains_partial_upload_and_later_chunks() {
    use bexos_debugd::service::UpdateManager;
    let mut old = BufferedUpdateManager::new();
    assert_eq!(
        block_on(old.begin_upload(UpdateUploadBeginRequest {
            upload_id: 71,
            manifest_len: 100,
            artifact_len: 50000
        }))
        .status,
        0
    );
    assert_eq!(
        block_on(old.write_chunk(UpdateChunkRequest {
            upload_id: 71,
            stream: 2,
            offset: 0,
            bytes: vec![3; 17000]
        }))
        .status,
        0
    );
    let mut new = BufferedUpdateManager::new();
    let keys = old.checkpoint_keys();
    for key in keys {
        new.adopt_record(key, old.checkpoint_record(key).unwrap().as_deref())
            .unwrap();
    }
    assert_eq!(old, new);
    let prior = old.checkpoint_keys();
    assert_eq!(
        block_on(old.write_chunk(UpdateChunkRequest {
            upload_id: 71,
            stream: 2,
            offset: 17000,
            bytes: vec![4; 33000]
        }))
        .status,
        0
    );
    let mut changes = prior;
    changes.extend(old.checkpoint_keys());
    changes.sort_unstable();
    changes.dedup();
    for key in changes {
        new.adopt_record(key, old.checkpoint_record(key).unwrap().as_deref())
            .unwrap();
    }
    assert_eq!(old, new);
    let mut bad = old.checkpoint_record(0).unwrap().unwrap();
    bad[0] = 255;
    assert!(new.adopt_record(0, Some(&bad)).is_err());
}

mod shell_auth_tests;
