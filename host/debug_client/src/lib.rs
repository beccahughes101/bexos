use std::io::ErrorKind;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use bexos_debug_wire::{
    AppBundleChunkRequest, AppBundleUploadBeginRequest, AppBundleUploadCommitRequest, AppInfo,
    AppInstallFromUrlRequest, AppLaunchRequest, AppUninstallRequest, ComponentConfigGetRequest,
    ComponentConfigGetResponse, ComponentConfigMutationResponse, ComponentConfigResetRequest,
    ComponentConfigSetRequest, DebugStatusResponse, ExecRequest, ExecResponse, Frame,
    HealthCheckResponse, METHOD_APPLY_UPDATE_FROM_FEED, METHOD_BEGIN_APP_BUNDLE_UPLOAD,
    METHOD_BEGIN_TEST_APP_UPLOAD, METHOD_BEGIN_UPDATE_UPLOAD, METHOD_CHECK_UPDATES,
    METHOD_COMMIT_APP_BUNDLE_UPLOAD, METHOD_COMMIT_TEST_APP_UPLOAD, METHOD_COMMIT_UPDATE_UPLOAD,
    METHOD_CREATE_USER, METHOD_DELETE_USER, METHOD_EXEC_COMMAND, METHOD_GET_COMPONENT_CONFIG,
    METHOD_GET_USER, METHOD_HEALTH_CHECK, METHOD_INSTALL_APP_FROM_URL, METHOD_LAUNCH_APP,
    METHOD_LAUNCH_TEST_APP, METHOD_LIST_APPS, METHOD_LIST_PROCESSES, METHOD_LIST_USERS,
    METHOD_LOCK_USER, METHOD_RELOAD_WELL_KNOWN, METHOD_RESET_COMPONENT_CONFIG,
    METHOD_SET_COMPONENT_CONFIG, METHOD_STAGE_UPDATE_FROM_FEED, METHOD_TEE_CLOSE_SESSION,
    METHOD_TEE_INFO, METHOD_TEE_INSTALL_APP, METHOD_TEE_INVOKE, METHOD_TEE_LIST_APPS,
    METHOD_TEE_OPEN_SESSION, METHOD_TEE_UNINSTALL_APP, METHOD_TEE_UPDATE_CORE,
    METHOD_TEE_UPDATE_STATUS, METHOD_TRACE_START, METHOD_TRACE_STATUS, METHOD_TRACE_STOP,
    METHOD_UNINSTALL_APP, METHOD_UNLOCK_USER, METHOD_UPDATE_USER, METHOD_WRITE_APP_BUNDLE_CHUNK,
    METHOD_WRITE_TEST_APP_CHUNK, METHOD_WRITE_UPDATE_CHUNK, ProcessInfo, TeeAppInfo,
    TeeInfoResponse, TeeInstallAppRequest, TeeInstallAppResponse, TeeInvokeRequest,
    TeeInvokeResponse, TeeOpenSessionResponse, TeeSessionRequest, TeeUpdateCoreRequest,
    TeeUpdateCoreResponse, TeeUpdateStatusResponse, TeeUuidRequest, TestAppChunkRequest,
    TestAppLaunchRequest, TestAppUploadBeginRequest, TestAppUploadCommitRequest, TraceStartRequest,
    TraceStatusResponse, TraceStopRequest, UpdateCheckRequest, UpdateCheckResponse,
    UpdateChunkRequest, UpdateUploadBeginRequest, UpdateUploadCommitRequest, UserCreateRequest,
    UserDeleteRequest, UserGetRequest, UserInfo, UserLockRequest, UserUnlockRequest,
    UserUpdateRequest, WellKnownReloadRequest, WireError, decode_app_list,
    decode_component_config_get_response, decode_component_config_mutation_response,
    decode_debug_status, decode_exec_response, decode_health_response, decode_process_list,
    decode_tee_app_list_response, decode_tee_info_response, decode_tee_install_app_response,
    decode_tee_invoke_response, decode_tee_open_session_response, decode_tee_update_core_response,
    decode_tee_update_status_response, decode_trace_status_response, decode_trace_stop_response,
    decode_update_check_response, decode_user_get_response, decode_user_list_response,
    encode_app_bundle_chunk, encode_app_bundle_upload_begin, encode_app_bundle_upload_commit,
    encode_app_install_from_url, encode_app_launch, encode_app_uninstall,
    encode_component_config_get, encode_component_config_reset, encode_component_config_set,
    encode_empty, encode_exec_request, encode_tee_install_app_request, encode_tee_invoke_request,
    encode_tee_session_request, encode_tee_update_core_request, encode_tee_uuid_request,
    encode_test_app_chunk, encode_test_app_launch, encode_test_app_upload_begin,
    encode_test_app_upload_commit, encode_trace_start_request, encode_trace_stop_request,
    encode_update_check_request, encode_update_chunk, encode_update_upload_begin,
    encode_update_upload_commit, encode_user_create_request, encode_user_delete_request,
    encode_user_get_request, encode_user_lock_request, encode_user_unlock_request,
    encode_user_update_request, encode_well_known_reload, parse_frame,
};

mod error;
mod transport;
pub use error::DebugClientError;
pub use transport::{DebugTransport, UnixSocketTransport};
mod apps;
mod config;
mod diagnostics;
mod framing;
mod tee;
mod trace;
mod updates;
mod uploads;
mod users;

pub struct DebugClient<T> {
    transport: T,
    next_request_id: u32,
    read_buffer: Vec<u8>,
    pending_frames: Vec<Frame>,
    received_trace: Vec<u8>,
}

fn check_debug_status(status: i32, message: &str) -> Result<(), DebugClientError> {
    if status == 0 {
        Ok(())
    } else {
        Err(DebugClientError::RemoteStatus(DebugStatusResponse {
            status,
            message: message.to_string(),
        }))
    }
}

pub type UnixDebugClient = DebugClient<UnixSocketTransport>;

mod shell;
