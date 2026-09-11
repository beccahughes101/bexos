mod migration;
use alloc::string::String;
use alloc::vec::Vec;

use bexos_debug_wire::{
    AppBundleChunkRequest, AppBundleUploadBeginRequest, AppBundleUploadCommitRequest, AppInfo,
    AppInstallFromUrlRequest, AppLaunchRequest, AppUninstallRequest, ComponentConfigGetRequest,
    ComponentConfigGetResponse, ComponentConfigMutationResponse, ComponentConfigResetRequest,
    ComponentConfigSetRequest, DebugStatusResponse, ExecResponse, Frame, HealthCheckResponse,
    METHOD_APPLY_UPDATE_FROM_FEED, METHOD_BEGIN_APP_BUNDLE_UPLOAD, METHOD_BEGIN_TEST_APP_UPLOAD,
    METHOD_BEGIN_UPDATE_UPLOAD, METHOD_CHECK_UPDATES, METHOD_COMMIT_APP_BUNDLE_UPLOAD,
    METHOD_COMMIT_TEST_APP_UPLOAD, METHOD_COMMIT_UPDATE_UPLOAD, METHOD_CREATE_USER,
    METHOD_DELETE_USER, METHOD_EXEC_COMMAND, METHOD_GET_COMPONENT_CONFIG, METHOD_GET_USER,
    METHOD_HEALTH_CHECK, METHOD_INSTALL_APP_FROM_URL, METHOD_LAUNCH_APP, METHOD_LAUNCH_TEST_APP,
    METHOD_LIST_APPS, METHOD_LIST_PROCESSES, METHOD_LIST_USERS, METHOD_LOCK_USER,
    METHOD_RELOAD_WELL_KNOWN, METHOD_RESET_COMPONENT_CONFIG, METHOD_SET_COMPONENT_CONFIG,
    METHOD_STAGE_UPDATE_FROM_FEED, METHOD_TEE_CLOSE_SESSION, METHOD_TEE_INFO,
    METHOD_TEE_INSTALL_APP, METHOD_TEE_INVOKE, METHOD_TEE_LIST_APPS, METHOD_TEE_OPEN_SESSION,
    METHOD_TEE_UNINSTALL_APP, METHOD_TEE_UPDATE_CORE, METHOD_TEE_UPDATE_STATUS, METHOD_TRACE_START,
    METHOD_TRACE_STATUS, METHOD_TRACE_STOP, METHOD_UNINSTALL_APP, METHOD_UNLOCK_USER,
    METHOD_UPDATE_USER, METHOD_WRITE_APP_BUNDLE_CHUNK, METHOD_WRITE_TEST_APP_CHUNK,
    METHOD_WRITE_UPDATE_CHUNK, ProcessInfo, TeeAppInfo, TeeAppListResponse, TeeInfoResponse,
    TeeInstallAppRequest, TeeInstallAppResponse, TeeInvokeRequest, TeeInvokeResponse,
    TeeOpenSessionResponse, TeeSessionRequest, TeeUpdateCoreRequest, TeeUpdateCoreResponse,
    TeeUpdateStatusResponse, TeeUuidRequest, TestAppChunkRequest, TestAppLaunchRequest,
    TestAppUploadBeginRequest, TestAppUploadCommitRequest, TraceStartRequest, TraceStatusResponse,
    TraceStopRequest, TraceStopResponse, UpdateCandidateInfo, UpdateCheckRequest,
    UpdateCheckResponse, UserCreateRequest, UserDeleteRequest, UserGetRequest, UserInfo,
    UserLockRequest, UserUnlockRequest, UserUpdateRequest, WellKnownReloadRequest,
    decode_app_bundle_chunk, decode_app_bundle_upload_begin, decode_app_bundle_upload_commit,
    decode_app_install_from_url, decode_app_launch, decode_app_uninstall,
    decode_component_config_get, decode_component_config_reset, decode_component_config_set,
    decode_exec_request, decode_tee_install_app_request, decode_tee_invoke_request,
    decode_tee_session_request, decode_tee_update_core_request, decode_tee_uuid_request,
    decode_test_app_chunk, decode_test_app_launch, decode_test_app_upload_begin,
    decode_test_app_upload_commit, decode_trace_start_request, decode_trace_stop_request,
    decode_update_check_request, decode_update_chunk, decode_update_upload_begin,
    decode_update_upload_commit, decode_user_create_request, decode_user_delete_request,
    decode_user_get_request, decode_user_lock_request, decode_user_unlock_request,
    decode_user_update_request, decode_well_known_reload, encode_app_list,
    encode_component_config_get_response, encode_component_config_mutation_response,
    encode_debug_status, encode_empty, encode_exec_response, encode_health_response,
    encode_process_list, encode_tee_app_list_response, encode_tee_info_response,
    encode_tee_install_app_response, encode_tee_invoke_response, encode_tee_open_session_response,
    encode_tee_update_core_response, encode_tee_update_status_response,
    encode_trace_status_response, encode_trace_stop_response, encode_update_check_response,
    encode_user_get_response, encode_user_list_response,
};
use bexos_trace::{
    BufferMode, CATEGORY_DEBUG_SERVICE, TraceConfig, TraceEvent, TraceEventKind, TraceOutputFormat,
    TraceSession, TraceState,
};
use bexos_trusty_client::protocol::{AUTHMGR_BE_UUID, KEYMINT_UUID, ORCHESTRATOR_UUID};
use bexos_trusty_client::services::{
    KEYMINT_CMD_BEGIN, KEYMINT_CMD_DELETE_KEY, KEYMINT_CMD_FINISH, KEYMINT_CMD_GENERATE_KEY,
    KEYMINT_CMD_UPDATE_AAD, KeyMintAlgorithm, KeyMintPurpose, ORCHESTRATOR_CMD_GET_KERNEL_SLOT,
    decode_keymint_begin, decode_keymint_delete_key, decode_keymint_finish,
    decode_keymint_generated_key, decode_keymint_update_aad, decode_orchestrator_response,
    encode_keymint_begin, encode_keymint_delete_key, encode_keymint_finish,
    encode_keymint_generate_key, encode_keymint_update_aad, encode_orchestrator_request,
};
use bexos_update::{ArtifactKind, TrustedKey, UpdateError, UpdateManifest, verify_update};
use bexos_userspace::{Channel, Memory};
use kernel_fidl::Status as KernelStatus;
use tracing_fidl::{
    BufferMode as TraceFidlBufferMode, FidlDecode as TraceDecode, FidlEncode as TraceEncode,
    HandleRef as TraceHandleRef, TraceCategory, TraceControllerGetStatusRequest,
    TraceControllerGetStatusResponse, TraceControllerStartSessionRequest,
    TraceControllerStartSessionResponse, TraceControllerStopSessionRequest,
    TraceControllerStopSessionResponse, TraceOutputFormat as TraceFidlOutputFormat,
};

pub const VERSION: &str = "qemu-socket-v1";

pub async fn handle_frame(request: &Frame, processes: &[ProcessInfo]) -> Frame {
    let mut installer = UnsupportedTestAppInstaller;
    let mut apps = UnsupportedAppManager;
    let mut updates = UnsupportedUpdateManager;
    let mut platform = UnsupportedPlatformUpdateApplier;
    handle_frame_with_backends(
        request,
        processes,
        &mut installer,
        &mut apps,
        &mut updates,
        &mut platform,
    )
    .await
}

fn unsupported_install_response() -> DebugStatusResponse {
    DebugStatusResponse {
        status: -95,
        message: "debugd test-app install backend unavailable".into(),
    }
}

fn unsupported_app_response() -> DebugStatusResponse {
    DebugStatusResponse {
        status: -95,
        message: "debugd app lifecycle backend unavailable".into(),
    }
}

fn unsupported_update_response() -> DebugStatusResponse {
    DebugStatusResponse {
        status: -95,
        message: "debugd update backend unavailable".into(),
    }
}

fn unsupported_tee_response() -> DebugStatusResponse {
    DebugStatusResponse {
        status: -95,
        message: "debugd tee backend unavailable".into(),
    }
}

fn unsupported_user_response() -> DebugStatusResponse {
    DebugStatusResponse {
        status: -95,
        message: "debugd user backend unavailable".into(),
    }
}

fn ok_response(message: &str) -> DebugStatusResponse {
    DebugStatusResponse {
        status: 0,
        message: message.into(),
    }
}

fn debug_status(status: i32, message: &str) -> DebugStatusResponse {
    DebugStatusResponse {
        status,
        message: message.into(),
    }
}

pub async fn handle_frame_with_installer<I: TestAppInstaller>(
    request: &Frame,
    processes: &[ProcessInfo],
    installer: &mut I,
) -> Frame {
    let mut apps = UnsupportedAppManager;
    let mut updates = UnsupportedUpdateManager;
    let mut platform = UnsupportedPlatformUpdateApplier;
    handle_frame_with_backends(
        request,
        processes,
        installer,
        &mut apps,
        &mut updates,
        &mut platform,
    )
    .await
}

pub async fn handle_frame_with_backends<
    I: TestAppInstaller,
    A: AppManager,
    U: UpdateManager,
    P: PlatformUpdateApplier,
>(
    request: &Frame,
    processes: &[ProcessInfo],
    installer: &mut I,
    apps: &mut A,
    updates: &mut U,
    platform: &mut P,
) -> Frame {
    let mut users = UnsupportedUserManager;
    let mut tee = UnsupportedTeeManager;
    handle_frame_with_all_backends(
        request, processes, installer, apps, updates, platform, &mut users, &mut tee,
    )
    .await
}

pub async fn handle_frame_with_all_backends<
    I: TestAppInstaller,
    A: AppManager,
    U: UpdateManager,
    P: PlatformUpdateApplier,
    M: UserManager,
    T: TeeManager,
>(
    request: &Frame,
    processes: &[ProcessInfo],
    installer: &mut I,
    apps: &mut A,
    updates: &mut U,
    platform: &mut P,
    users: &mut M,
    tee: &mut T,
) -> Frame {
    let mut traces = UnsupportedTraceManager;
    handle_frame_with_trace_backends(
        request,
        processes,
        installer,
        apps,
        updates,
        platform,
        users,
        tee,
        &mut traces,
    )
    .await
}

pub async fn handle_frame_with_trace_backends<
    I: TestAppInstaller,
    A: AppManager,
    U: UpdateManager,
    P: PlatformUpdateApplier,
    M: UserManager,
    T: TeeManager,
    R: TraceManager,
>(
    request: &Frame,
    processes: &[ProcessInfo],
    installer: &mut I,
    apps: &mut A,
    updates: &mut U,
    platform: &mut P,
    users: &mut M,
    tee: &mut T,
    traces: &mut R,
) -> Frame {
    let mut payload = Vec::new();
    match request.method_id {
        METHOD_HEALTH_CHECK => {
            traces.record_debug_event("debugd:health_check").await;
            encode_health_response(
                &HealthCheckResponse {
                    service_name: "debugd".into(),
                    status: "SERVING".into(),
                    version: VERSION.into(),
                },
                &mut payload,
            );
        }
        METHOD_LIST_PROCESSES => {
            traces.record_debug_event("debugd:list_processes").await;
            encode_process_list(processes, &mut payload);
        }
        METHOD_LIST_APPS => match apps.list_apps().await {
            Ok(list) => {
                traces.record_debug_event("debugd:list_apps").await;
                encode_app_list(&list, &mut payload)
            }
            Err(response) => encode_debug_status(&response, &mut payload),
        },
        METHOD_LIST_USERS => {
            let response = match users.list_users().await {
                Ok(users) => bexos_debug_wire::UserListResponse { status: 0, users },
                Err(response) => bexos_debug_wire::UserListResponse {
                    status: response.status,
                    users: Vec::new(),
                },
            };
            encode_user_list_response(&response, &mut payload);
        }
        METHOD_GET_USER => {
            let response = match decode_user_get_request(&request.payload) {
                Ok(get) => match users.get_user(get).await {
                    Ok(user) => bexos_debug_wire::UserGetResponse { status: 0, user },
                    Err(response) => bexos_debug_wire::UserGetResponse {
                        status: response.status,
                        user: UserInfo::default(),
                    },
                },
                Err(_) => bexos_debug_wire::UserGetResponse {
                    status: -8,
                    user: UserInfo::default(),
                },
            };
            encode_user_get_response(&response, &mut payload);
        }
        METHOD_EXEC_COMMAND => {
            traces.record_debug_event("debugd:exec_command").await;
            bexos_userspace::log("debugd: exec command handling\n");
            let response = match decode_exec_request(&request.payload) {
                Ok(exec) => {
                    exec_command(
                        &exec.component_id,
                        &exec.args,
                        processes,
                        apps,
                        updates,
                        platform,
                        tee,
                    )
                    .await
                }
                Err(_) => ExecResponse {
                    exit_code: 2,
                    stdout: String::new(),
                    stderr: "invalid ExecRequest\n".into(),
                },
            };
            encode_exec_response(&response, &mut payload);
        }
        METHOD_BEGIN_TEST_APP_UPLOAD => {
            let response = match decode_test_app_upload_begin(&request.payload) {
                Ok(upload) => installer.begin_upload(upload).await,
                Err(_) => invalid_request_response("invalid BeginTestAppUpload request"),
            };
            encode_debug_status(&response, &mut payload);
        }
        METHOD_WRITE_TEST_APP_CHUNK => {
            let response = match decode_test_app_chunk(&request.payload) {
                Ok(chunk) => installer.write_chunk(chunk).await,
                Err(_) => invalid_request_response("invalid WriteTestAppChunk request"),
            };
            encode_debug_status(&response, &mut payload);
        }
        METHOD_COMMIT_TEST_APP_UPLOAD => {
            let response = match decode_test_app_upload_commit(&request.payload) {
                Ok(commit) => installer.commit_upload(commit).await,
                Err(_) => invalid_request_response("invalid CommitTestAppUpload request"),
            };
            encode_debug_status(&response, &mut payload);
        }
        METHOD_LAUNCH_TEST_APP => {
            let response = match decode_test_app_launch(&request.payload) {
                Ok(launch) => installer.launch(launch).await,
                Err(_) => invalid_request_response("invalid LaunchTestApp request"),
            };
            encode_debug_status(&response, &mut payload);
        }
        METHOD_BEGIN_APP_BUNDLE_UPLOAD => {
            let response = match decode_app_bundle_upload_begin(&request.payload) {
                Ok(upload) => apps.begin_bundle_upload(upload).await,
                Err(_) => invalid_request_response("invalid BeginAppBundleUpload request"),
            };
            encode_debug_status(&response, &mut payload);
        }
        METHOD_WRITE_APP_BUNDLE_CHUNK => {
            let response = match decode_app_bundle_chunk(&request.payload) {
                Ok(chunk) => apps.write_bundle_chunk(chunk).await,
                Err(_) => invalid_request_response("invalid WriteAppBundleChunk request"),
            };
            encode_debug_status(&response, &mut payload);
        }
        METHOD_COMMIT_APP_BUNDLE_UPLOAD => {
            let response = match decode_app_bundle_upload_commit(&request.payload) {
                Ok(commit) => apps.commit_bundle_upload(commit).await,
                Err(_) => invalid_request_response("invalid CommitAppBundleUpload request"),
            };
            encode_debug_status(&response, &mut payload);
        }
        METHOD_UNINSTALL_APP => {
            let response = match decode_app_uninstall(&request.payload) {
                Ok(uninstall) => apps.uninstall(uninstall).await,
                Err(_) => invalid_request_response("invalid UninstallApp request"),
            };
            encode_debug_status(&response, &mut payload);
        }
        METHOD_LAUNCH_APP => {
            let response = match decode_app_launch(&request.payload) {
                Ok(launch) => apps.launch(launch).await,
                Err(_) => invalid_request_response("invalid LaunchApp request"),
            };
            encode_debug_status(&response, &mut payload);
        }
        METHOD_INSTALL_APP_FROM_URL => {
            let response = match decode_app_install_from_url(&request.payload) {
                Ok(install) => apps.install_from_url(install).await,
                Err(_) => invalid_request_response("invalid InstallAppFromUrl request"),
            };
            encode_debug_status(&response, &mut payload);
        }
        METHOD_RELOAD_WELL_KNOWN => {
            let response = match decode_well_known_reload(&request.payload) {
                Ok(reload) => apps.reload_well_known(reload).await,
                Err(_) => invalid_request_response("invalid ReloadWellKnown request"),
            };
            encode_debug_status(&response, &mut payload);
        }
        bexos_debug_wire::METHOD_PREFERENCES => {
            let response = match bexos_debug_wire::decode_preferences_request(&request.payload) {
                Ok(q) => apps.preferences(q).await,
                Err(_) => bexos_debug_wire::PreferencesResponse {
                    status: -8,
                    ..Default::default()
                },
            };
            bexos_debug_wire::encode_preferences_response(&response, &mut payload);
        }
        METHOD_GET_COMPONENT_CONFIG => {
            traces
                .record_debug_event("debugd:get_component_config")
                .await;
            let response = match decode_component_config_get(&request.payload) {
                Ok(get) => apps.get_component_config(get).await,
                Err(_) => ComponentConfigGetResponse {
                    status: -8,
                    generation: 0,
                    config: Vec::new(),
                },
            };
            encode_component_config_get_response(&response, &mut payload);
        }
        METHOD_SET_COMPONENT_CONFIG => {
            traces
                .record_debug_event("debugd:set_component_config")
                .await;
            let response = match decode_component_config_set(&request.payload) {
                Ok(set) => apps.set_component_config(set).await,
                Err(_) => ComponentConfigMutationResponse {
                    status: -8,
                    generation: 0,
                    message: "invalid SetComponentConfig request".into(),
                },
            };
            encode_component_config_mutation_response(&response, &mut payload);
        }
        METHOD_RESET_COMPONENT_CONFIG => {
            traces
                .record_debug_event("debugd:reset_component_config")
                .await;
            let response = match decode_component_config_reset(&request.payload) {
                Ok(reset) => apps.reset_component_config(reset).await,
                Err(_) => ComponentConfigMutationResponse {
                    status: -8,
                    generation: 0,
                    message: "invalid ResetComponentConfig request".into(),
                },
            };
            encode_component_config_mutation_response(&response, &mut payload);
        }
        METHOD_CREATE_USER => {
            let response = match decode_user_create_request(&request.payload) {
                Ok(create) => users.create_user(create).await,
                Err(_) => invalid_request_response("invalid CreateUser request"),
            };
            encode_debug_status(&response, &mut payload);
        }
        METHOD_UPDATE_USER => {
            let response = match decode_user_update_request(&request.payload) {
                Ok(update) => users.update_user(update).await,
                Err(_) => invalid_request_response("invalid UpdateUser request"),
            };
            encode_debug_status(&response, &mut payload);
        }
        METHOD_DELETE_USER => {
            let response = match decode_user_delete_request(&request.payload) {
                Ok(delete) => users.delete_user(delete).await,
                Err(_) => invalid_request_response("invalid DeleteUser request"),
            };
            encode_debug_status(&response, &mut payload);
        }
        METHOD_UNLOCK_USER => {
            let response = match decode_user_unlock_request(&request.payload) {
                Ok(unlock) => users.unlock_user(unlock).await,
                Err(_) => invalid_request_response("invalid UnlockUser request"),
            };
            encode_debug_status(&response, &mut payload);
        }
        METHOD_LOCK_USER => {
            let response = match decode_user_lock_request(&request.payload) {
                Ok(lock) => users.lock_user(lock).await,
                Err(_) => invalid_request_response("invalid LockUser request"),
            };
            encode_debug_status(&response, &mut payload);
        }
        METHOD_TEE_INFO => encode_tee_info_response(&tee.info().await, &mut payload),
        METHOD_TEE_LIST_APPS => encode_tee_app_list_response(&tee.list_apps().await, &mut payload),
        METHOD_TEE_INSTALL_APP => {
            let response = match decode_tee_install_app_request(&request.payload) {
                Ok(request) => tee.install_app(request).await,
                Err(_) => TeeInstallAppResponse {
                    status: -8,
                    uuid: Vec::new(),
                },
            };
            encode_tee_install_app_response(&response, &mut payload);
        }
        METHOD_TEE_UNINSTALL_APP => {
            let response = match decode_tee_uuid_request(&request.payload) {
                Ok(request) => tee.uninstall_app(request).await,
                Err(_) => invalid_request_response("invalid TeeUninstallApp request"),
            };
            encode_debug_status(&response, &mut payload);
        }
        METHOD_TEE_OPEN_SESSION => {
            let response = match decode_tee_uuid_request(&request.payload) {
                Ok(request) => tee.open_session(request).await,
                Err(_) => TeeOpenSessionResponse {
                    status: -8,
                    session_id: 0,
                },
            };
            encode_tee_open_session_response(&response, &mut payload);
        }
        METHOD_TEE_CLOSE_SESSION => {
            let response = match decode_tee_session_request(&request.payload) {
                Ok(request) => tee.close_session(request).await,
                Err(_) => invalid_request_response("invalid TeeCloseSession request"),
            };
            encode_debug_status(&response, &mut payload);
        }
        METHOD_TEE_INVOKE => {
            let response = match decode_tee_invoke_request(&request.payload) {
                Ok(request) => tee.invoke(request).await,
                Err(_) => TeeInvokeResponse {
                    status: -8,
                    response: Vec::new(),
                },
            };
            encode_tee_invoke_response(&response, &mut payload);
        }
        METHOD_TEE_UPDATE_CORE => {
            let response = match decode_tee_update_core_request(&request.payload) {
                Ok(request) => tee.update_core(request).await,
                Err(_) => TeeUpdateCoreResponse {
                    status: -8,
                    new_version: 0,
                    message: "invalid TeeUpdateCore request".into(),
                },
            };
            encode_tee_update_core_response(&response, &mut payload);
        }
        METHOD_TEE_UPDATE_STATUS => {
            encode_tee_update_status_response(&tee.update_status().await, &mut payload);
        }
        METHOD_TRACE_START => {
            let response = match decode_trace_start_request(&request.payload) {
                Ok(start) => traces.start_trace(start).await,
                Err(_) => invalid_request_response("invalid TraceStart request"),
            };
            encode_debug_status(&response, &mut payload);
        }
        METHOD_TRACE_STATUS => {
            encode_trace_status_response(&traces.trace_status().await, &mut payload);
        }
        METHOD_TRACE_STOP => {
            let response = match decode_trace_stop_request(&request.payload) {
                Ok(stop) => traces.stop_trace(stop).await,
                Err(_) => TraceStopResponse {
                    status: -8,
                    ..TraceStopResponse::default()
                },
            };
            encode_trace_stop_response(&response, &mut payload);
        }
        METHOD_BEGIN_UPDATE_UPLOAD => {
            let response = match decode_update_upload_begin(&request.payload) {
                Ok(upload) => updates.begin_upload(upload).await,
                Err(_) => invalid_request_response("invalid BeginUpdateUpload request"),
            };
            encode_debug_status(&response, &mut payload);
        }
        METHOD_WRITE_UPDATE_CHUNK => {
            let response = match decode_update_chunk(&request.payload) {
                Ok(chunk) => updates.write_chunk(chunk).await,
                Err(_) => invalid_request_response("invalid WriteUpdateChunk request"),
            };
            encode_debug_status(&response, &mut payload);
        }
        METHOD_COMMIT_UPDATE_UPLOAD => {
            let response = match decode_update_upload_commit(&request.payload) {
                Ok(commit) => updates.commit_upload(commit).await,
                Err(_) => invalid_request_response("invalid CommitUpdateUpload request"),
            };
            encode_debug_status(&response, &mut payload);
        }
        METHOD_CHECK_UPDATES => {
            let response = match decode_update_check_request(&request.payload) {
                Ok(check) => updates.check_updates(check).await,
                Err(_) => UpdateCheckResponse {
                    status: -8,
                    message: "invalid UpdateCheck request".into(),
                    candidates: Vec::new(),
                },
            };
            encode_update_check_response(&response, &mut payload);
        }
        METHOD_STAGE_UPDATE_FROM_FEED => {
            let response = match decode_update_check_request(&request.payload) {
                Ok(check) => updates.stage_from_feed(check).await,
                Err(_) => invalid_request_response("invalid UpdateCheck request"),
            };
            encode_debug_status(&response, &mut payload);
        }
        METHOD_APPLY_UPDATE_FROM_FEED => {
            let response = match decode_update_check_request(&request.payload) {
                Ok(check) => updates.apply_from_feed(check).await,
                Err(_) => invalid_request_response("invalid UpdateCheck request"),
            };
            encode_debug_status(&response, &mut payload);
        }
        _ => {
            encode_exec_response(
                &ExecResponse {
                    exit_code: 127,
                    stdout: String::new(),
                    stderr: "unknown debug method\n".into(),
                },
                &mut payload,
            );
        }
    }
    Frame {
        flags: request.flags,
        request_id: request.request_id,
        method_id: request.method_id,
        payload,
    }
}

fn invalid_request_response(message: &str) -> DebugStatusResponse {
    DebugStatusResponse {
        status: -8,
        message: message.into(),
    }
}

fn derive_debug_uuid(payload: &[u8]) -> [u8; 16] {
    let mut uuid = *b"BEXOS-TEE-DBG-v1";
    for (index, byte) in payload.iter().enumerate() {
        uuid[index % 16] = uuid[index % 16].rotate_left(1) ^ *byte;
    }
    uuid
}

fn hex_uuid(bytes: &[u8]) -> String {
    let mut out = String::new();
    for (index, byte) in bytes.iter().enumerate() {
        if matches!(index, 4 | 6 | 8 | 10) {
            out.push('-');
        }
        out.push_str(&alloc::format!("{byte:02x}"));
    }
    out
}

fn status_exec(status: DebugStatusResponse) -> ExecResponse {
    if status.status == 0 {
        ExecResponse {
            exit_code: 0,
            stdout: alloc::format!("{}\n", status.message),
            stderr: String::new(),
        }
    } else {
        ExecResponse {
            exit_code: status.status,
            stdout: String::new(),
            stderr: alloc::format!("{}\n", status.message),
        }
    }
}

fn qemu_trusted_update_keys() -> [TrustedKey<'static>; 1] {
    [TrustedKey {
        key_id: *b"bexos-qemu-test-ed25519-key-v001",
        public_key: &[
            0xd7, 0x5a, 0x98, 0x01, 0x82, 0xb1, 0x0a, 0xb7, 0xd5, 0x4b, 0xfe, 0xd3, 0xc9, 0x64,
            0x07, 0x3a, 0x0e, 0xe1, 0x72, 0xf3, 0xda, 0xa6, 0x23, 0x25, 0xaf, 0x02, 0x1a, 0x68,
            0xf7, 0x07, 0x51, 0x1a,
        ],
    }]
}

pub fn empty_response(request: &Frame) -> Frame {
    let mut payload = Vec::new();
    encode_empty(&mut payload);
    Frame {
        flags: request.flags,
        request_id: request.request_id,
        method_id: request.method_id,
        payload,
    }
}

mod test_apps;
pub use test_apps::*;

mod apps;
pub use apps::*;

mod users;
pub use users::*;

mod platform;
pub use platform::*;

mod tee;
pub use tee::*;

mod trace;
pub use trace::*;

mod updates;
pub use updates::*;

mod buffered_apps;
pub use buffered_apps::*;

mod buffered_users;
pub use buffered_users::*;

mod buffered_test_apps;
pub use buffered_test_apps::*;

mod diagnostics;
use diagnostics::*;
