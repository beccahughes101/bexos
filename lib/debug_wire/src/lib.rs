#![no_std]

extern crate alloc;
mod preferences;
pub use preferences::*;

use alloc::string::{String, ToString};
use alloc::vec::Vec;

pub const FRAME_MAGIC: [u8; 4] = *b"BXD1";
pub const FRAME_VERSION: u16 = 1;
pub const HEADER_LEN: usize = 20;
pub const MAX_PAYLOAD_LEN: usize = 64 * 1024;

pub const METHOD_HEALTH_CHECK: u32 = 1;
pub const METHOD_EXEC_COMMAND: u32 = 2;
pub const METHOD_LIST_PROCESSES: u32 = 3;
pub const METHOD_BEGIN_TEST_APP_UPLOAD: u32 = 16;
pub const METHOD_WRITE_TEST_APP_CHUNK: u32 = 17;
pub const METHOD_COMMIT_TEST_APP_UPLOAD: u32 = 18;
pub const METHOD_LAUNCH_TEST_APP: u32 = 19;
pub const METHOD_LIST_APPS: u32 = 32;
pub const METHOD_BEGIN_APP_BUNDLE_UPLOAD: u32 = 33;
pub const METHOD_WRITE_APP_BUNDLE_CHUNK: u32 = 34;
pub const METHOD_COMMIT_APP_BUNDLE_UPLOAD: u32 = 35;
pub const METHOD_UNINSTALL_APP: u32 = 36;
pub const METHOD_LAUNCH_APP: u32 = 37;
pub const METHOD_INSTALL_APP_FROM_URL: u32 = 38;
pub const METHOD_RELOAD_WELL_KNOWN: u32 = 39;
pub const METHOD_GET_COMPONENT_CONFIG: u32 = 40;
pub const METHOD_SET_COMPONENT_CONFIG: u32 = 41;
pub const METHOD_RESET_COMPONENT_CONFIG: u32 = 42;
pub const METHOD_BEGIN_UPDATE_UPLOAD: u32 = 48;
pub const METHOD_WRITE_UPDATE_CHUNK: u32 = 49;
pub const METHOD_COMMIT_UPDATE_UPLOAD: u32 = 50;
pub const METHOD_CHECK_UPDATES: u32 = 51;
pub const METHOD_STAGE_UPDATE_FROM_FEED: u32 = 52;
pub const METHOD_APPLY_UPDATE_FROM_FEED: u32 = 53;
pub const METHOD_LIST_USERS: u32 = 64;
pub const METHOD_GET_USER: u32 = 65;
pub const METHOD_CREATE_USER: u32 = 66;
pub const METHOD_UPDATE_USER: u32 = 67;
pub const METHOD_DELETE_USER: u32 = 68;
pub const METHOD_UNLOCK_USER: u32 = 69;
pub const METHOD_LOCK_USER: u32 = 70;
pub const METHOD_TEE_INFO: u32 = 80;
pub const METHOD_TEE_LIST_APPS: u32 = 81;
pub const METHOD_TEE_INSTALL_APP: u32 = 82;
pub const METHOD_TEE_UNINSTALL_APP: u32 = 83;
pub const METHOD_TEE_OPEN_SESSION: u32 = 84;
pub const METHOD_TEE_CLOSE_SESSION: u32 = 85;
pub const METHOD_TEE_INVOKE: u32 = 86;
pub const METHOD_TEE_UPDATE_CORE: u32 = 87;
pub const METHOD_TEE_UPDATE_STATUS: u32 = 88;
pub const METHOD_TRACE_START: u32 = 96;
pub const METHOD_TRACE_STATUS: u32 = 97;
pub const METHOD_TRACE_STOP: u32 = 98;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WireError {
    InvalidFrame,
    UnsupportedVersion,
    PayloadTooLarge,
    Incomplete,
    InvalidProto,
    Utf8,
}

impl core::fmt::Display for WireError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::InvalidFrame => "invalid frame header",
            Self::UnsupportedVersion => "unsupported frame version",
            Self::PayloadTooLarge => "payload exceeds the protocol limit",
            Self::Incomplete => "incomplete frame",
            Self::InvalidProto => "malformed protobuf payload",
            Self::Utf8 => "invalid UTF-8 text",
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Frame {
    pub flags: u16,
    pub request_id: u32,
    pub method_id: u32,
    pub payload: Vec<u8>,
}

impl Frame {
    pub fn encode(&self, out: &mut Vec<u8>) -> Result<(), WireError> {
        if self.payload.len() > MAX_PAYLOAD_LEN {
            return Err(WireError::PayloadTooLarge);
        }
        out.extend_from_slice(&FRAME_MAGIC);
        out.extend_from_slice(&FRAME_VERSION.to_le_bytes());
        out.extend_from_slice(&self.flags.to_le_bytes());
        out.extend_from_slice(&self.request_id.to_le_bytes());
        out.extend_from_slice(&self.method_id.to_le_bytes());
        out.extend_from_slice(&(self.payload.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.payload);
        Ok(())
    }
}

pub fn parse_frame(input: &[u8]) -> Result<(Frame, usize), WireError> {
    let start = find_magic(input).ok_or(WireError::Incomplete)?;
    if input.len() - start < HEADER_LEN {
        return Err(WireError::Incomplete);
    }
    let header = &input[start..start + HEADER_LEN];
    let version = u16::from_le_bytes([header[4], header[5]]);
    if version != FRAME_VERSION {
        return Err(WireError::UnsupportedVersion);
    }
    let flags = u16::from_le_bytes([header[6], header[7]]);
    let request_id = u32::from_le_bytes([header[8], header[9], header[10], header[11]]);
    let method_id = u32::from_le_bytes([header[12], header[13], header[14], header[15]]);
    let payload_len = u32::from_le_bytes([header[16], header[17], header[18], header[19]]) as usize;
    if payload_len > MAX_PAYLOAD_LEN {
        return Err(WireError::PayloadTooLarge);
    }
    let end = start + HEADER_LEN + payload_len;
    if input.len() < end {
        return Err(WireError::Incomplete);
    }
    Ok((
        Frame {
            flags,
            request_id,
            method_id,
            payload: input[start + HEADER_LEN..end].to_vec(),
        },
        end,
    ))
}

fn find_magic(input: &[u8]) -> Option<usize> {
    input
        .windows(FRAME_MAGIC.len())
        .position(|window| window == FRAME_MAGIC)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HealthCheckResponse {
    pub service_name: String,
    pub status: String,
    pub version: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecRequest {
    pub component_id: String,
    pub args: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecResponse {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProcessInfo {
    pub main_thread_id: u64,
    pub resource_group_id: Option<u32>,
    pub resource_group_name: String,
    pub parent_resource_group_id: Option<u32>,
    pub parent_resource_group_name: String,
    pub pid: u64,
    pub name: String,
    pub state: String,
    pub package_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppInfo {
    pub package_id: String,
    pub name: String,
    pub state: String,
    pub source: String,
    pub protected: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TestAppUploadBeginRequest {
    pub upload_id: u64,
    pub package_id: String,
    pub manifest_len: u64,
    pub elf_len: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TestAppChunkRequest {
    pub upload_id: u64,
    pub stream: u32,
    pub offset: u64,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TestAppUploadCommitRequest {
    pub upload_id: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TestAppLaunchRequest {
    pub package_id: String,
    pub process_name: String,
    pub arg0: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DebugStatusResponse {
    pub status: i32,
    pub message: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AppBundleUploadBeginRequest {
    pub upload_id: u64,
    pub archive_len: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AppBundleChunkRequest {
    pub upload_id: u64,
    pub offset: u64,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AppBundleUploadCommitRequest {
    pub upload_id: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AppUninstallRequest {
    pub package_id: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AppLaunchRequest {
    pub package_id: String,
    pub process_name: String,
    pub arg0: u64,
    pub uid: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AppInstallFromUrlRequest {
    pub url: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ComponentConfigGetRequest {
    pub package_id: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ComponentConfigGetResponse {
    pub status: i32,
    pub generation: u64,
    pub config: Vec<u8>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ComponentConfigSetRequest {
    pub package_id: String,
    pub expected_generation: u64,
    pub config: Vec<u8>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ComponentConfigResetRequest {
    pub package_id: String,
    pub expected_generation: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ComponentConfigMutationResponse {
    pub status: i32,
    pub generation: u64,
    pub message: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WellKnownReloadRequest {
    pub domain: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UpdateUploadBeginRequest {
    pub upload_id: u64,
    pub manifest_len: u64,
    pub artifact_len: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UpdateChunkRequest {
    pub upload_id: u64,
    pub stream: u32,
    pub offset: u64,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UpdateUploadCommitRequest {
    pub upload_id: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UpdateCheckRequest {
    pub selector_kind: u32,
    pub target: String,
    pub all: bool,
    pub stage: bool,
    pub apply: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UpdateCandidateInfo {
    pub target: String,
    pub path: String,
    pub url: String,
    pub kind: u32,
    pub generation: u64,
    pub length: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UpdateCheckResponse {
    pub status: i32,
    pub message: String,
    pub candidates: Vec<UpdateCandidateInfo>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UserInfo {
    pub uid: u64,
    pub name: String,
    pub display_name: String,
    pub disabled: bool,
    pub home_path: String,
    pub unlocked: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UserListResponse {
    pub status: i32,
    pub users: Vec<UserInfo>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UserGetRequest {
    pub uid: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UserGetResponse {
    pub status: i32,
    pub user: UserInfo,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UserCreateRequest {
    pub uid: u64,
    pub name: String,
    pub display_name: String,
    pub password: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UserUpdateRequest {
    pub uid: u64,
    pub name: String,
    pub display_name: String,
    pub disabled: bool,
    pub current_password: String,
    pub new_password: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UserDeleteRequest {
    pub uid: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UserUnlockRequest {
    pub uid: u64,
    pub password: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UserLockRequest {
    pub uid: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TeeInfoResponse {
    pub status: i32,
    pub present: bool,
    pub kind: String,
    pub secure_os_version: u32,
    pub anti_rollback_version: u32,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TeeAppInfo {
    pub uuid: Vec<u8>,
    pub version: u32,
    pub active_sessions: u32,
    pub entry_point_name: String,
    pub storage_bytes_used: u64,
    pub package_id: String,
    pub protected: bool,
    pub package_managed: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TeeAppListResponse {
    pub status: i32,
    pub apps: Vec<TeeAppInfo>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TeeInstallAppRequest {
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TeeInstallAppResponse {
    pub status: i32,
    pub uuid: Vec<u8>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TeeUuidRequest {
    pub uuid: Vec<u8>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TeeSessionRequest {
    pub session_id: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TeeOpenSessionResponse {
    pub status: i32,
    pub session_id: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TeeInvokeRequest {
    pub session_id: u64,
    pub command_id: u32,
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TeeInvokeResponse {
    pub status: i32,
    pub response: Vec<u8>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TeeUpdateCoreRequest {
    pub generation: u64,
    pub target: String,
    pub artifact_hash: Vec<u8>,
    pub image: Vec<u8>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TeeUpdateCoreResponse {
    pub status: i32,
    pub new_version: u32,
    pub message: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TeeUpdateStatusResponse {
    pub status: i32,
    pub update_status: String,
    pub generation: u64,
    pub message: String,
    pub phase: String,
    pub active_slot: String,
    pub pending_slot: String,
    pub reboot_required: bool,
    pub rollback_available: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TraceStartRequest {
    pub categories: u32,
    pub buffer_mode: u32,
    pub buffer_size_kb: u32,
    pub output_format: u32,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TraceStatusResponse {
    pub status: i32,
    pub state: u32,
    pub categories: u32,
    pub buffer_mode: u32,
    pub buffer_size_kb: u32,
    pub output_format: u32,
    pub producer_count: u32,
    pub event_count: u64,
    pub dropped_count: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TraceStopRequest {
    pub offset: u64,
    pub max_bytes: u32,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TraceStopResponse {
    pub status: i32,
    pub offset: u64,
    pub total_len: u64,
    pub bytes: Vec<u8>,
    pub complete: bool,
    pub output_format: u32,
}

pub fn encode_empty(out: &mut Vec<u8>) {
    out.clear();
}

pub fn encode_health_response(value: &HealthCheckResponse, out: &mut Vec<u8>) {
    out.clear();
    put_string(out, 1, &value.service_name);
    put_string(out, 2, &value.status);
    put_string(out, 3, &value.version);
}

pub fn decode_health_response(input: &[u8]) -> Result<HealthCheckResponse, WireError> {
    let mut response = HealthCheckResponse {
        service_name: String::new(),
        status: String::new(),
        version: String::new(),
    };
    read_fields(input, |field, wire, value| {
        if wire != 2 {
            return Ok(());
        }
        match field {
            1 => response.service_name = read_string(value)?,
            2 => response.status = read_string(value)?,
            3 => response.version = read_string(value)?,
            _ => {}
        }
        Ok(())
    })?;
    Ok(response)
}

pub fn encode_exec_request(value: &ExecRequest, out: &mut Vec<u8>) {
    out.clear();
    put_string(out, 1, &value.component_id);
    for arg in &value.args {
        put_string(out, 2, arg);
    }
}

pub fn decode_exec_request(input: &[u8]) -> Result<ExecRequest, WireError> {
    let mut request = ExecRequest {
        component_id: String::new(),
        args: Vec::new(),
    };
    read_fields(input, |field, wire, value| {
        if wire != 2 {
            return Ok(());
        }
        match field {
            1 => request.component_id = read_string(value)?,
            2 => request.args.push(read_string(value)?),
            _ => {}
        }
        Ok(())
    })?;
    Ok(request)
}

pub fn encode_exec_response(value: &ExecResponse, out: &mut Vec<u8>) {
    out.clear();
    put_varint_field(out, 1, value.exit_code as u64);
    put_string(out, 2, &value.stdout);
    put_string(out, 3, &value.stderr);
}

pub fn decode_exec_response(input: &[u8]) -> Result<ExecResponse, WireError> {
    let mut response = ExecResponse {
        exit_code: 0,
        stdout: String::new(),
        stderr: String::new(),
    };
    read_fields(input, |field, wire, value| {
        match (field, wire) {
            (1, 0) => response.exit_code = read_varint_from(value)? as i32,
            (2, 2) => response.stdout = read_string(value)?,
            (3, 2) => response.stderr = read_string(value)?,
            _ => {}
        }
        Ok(())
    })?;
    Ok(response)
}

pub fn encode_process_list(processes: &[ProcessInfo], out: &mut Vec<u8>) {
    out.clear();
    let mut item = Vec::new();
    for process in processes {
        item.clear();
        put_varint_field(&mut item, 1, process.pid);
        put_string(&mut item, 2, &process.name);
        put_string(&mut item, 3, &process.state);
        put_string(&mut item, 4, &process.package_id);
        put_varint_field(&mut item, 5, process.main_thread_id);
        if let Some(id) = process.resource_group_id {
            put_varint_field(&mut item, 6, id as u64);
        }
        put_string(&mut item, 7, &process.resource_group_name);
        if let Some(id) = process.parent_resource_group_id {
            put_varint_field(&mut item, 8, id as u64);
        }
        put_string(&mut item, 9, &process.parent_resource_group_name);
        put_len_field(out, 1, &item);
    }
}

pub fn decode_process_list(input: &[u8]) -> Result<Vec<ProcessInfo>, WireError> {
    let mut processes = Vec::new();
    read_fields(input, |field, wire, value| {
        if field == 1 && wire == 2 {
            processes.push(decode_process(value)?);
        }
        Ok(())
    })?;
    Ok(processes)
}

pub fn encode_app_list(apps: &[AppInfo], out: &mut Vec<u8>) {
    out.clear();
    let mut item = Vec::new();
    for app in apps {
        item.clear();
        put_string(&mut item, 1, &app.package_id);
        put_string(&mut item, 2, &app.name);
        put_string(&mut item, 3, &app.state);
        put_string(&mut item, 4, &app.source);
        put_varint_field(&mut item, 5, app.protected as u64);
        put_len_field(out, 1, &item);
    }
}

pub fn decode_app_list(input: &[u8]) -> Result<Vec<AppInfo>, WireError> {
    let mut apps = Vec::new();
    read_fields(input, |field, wire, value| {
        if field == 1 && wire == 2 {
            apps.push(decode_app(value)?);
        }
        Ok(())
    })?;
    Ok(apps)
}

pub fn encode_test_app_upload_begin(value: &TestAppUploadBeginRequest, out: &mut Vec<u8>) {
    out.clear();
    put_varint_field(out, 1, value.upload_id);
    put_string(out, 2, &value.package_id);
    put_varint_field(out, 3, value.manifest_len);
    put_varint_field(out, 4, value.elf_len);
}

pub fn decode_test_app_upload_begin(input: &[u8]) -> Result<TestAppUploadBeginRequest, WireError> {
    let mut request = TestAppUploadBeginRequest::default();
    read_fields(input, |field, wire, value| {
        match (field, wire) {
            (1, 0) => request.upload_id = read_varint_from(value)?,
            (2, 2) => request.package_id = read_string(value)?,
            (3, 0) => request.manifest_len = read_varint_from(value)?,
            (4, 0) => request.elf_len = read_varint_from(value)?,
            _ => {}
        }
        Ok(())
    })?;
    Ok(request)
}

pub fn encode_test_app_chunk(value: &TestAppChunkRequest, out: &mut Vec<u8>) {
    out.clear();
    put_varint_field(out, 1, value.upload_id);
    put_varint_field(out, 2, value.stream as u64);
    put_varint_field(out, 3, value.offset);
    put_len_field(out, 4, &value.bytes);
}

pub fn decode_test_app_chunk(input: &[u8]) -> Result<TestAppChunkRequest, WireError> {
    let mut request = TestAppChunkRequest::default();
    read_fields(input, |field, wire, value| {
        match (field, wire) {
            (1, 0) => request.upload_id = read_varint_from(value)?,
            (2, 0) => request.stream = read_varint_from(value)? as u32,
            (3, 0) => request.offset = read_varint_from(value)?,
            (4, 2) => request.bytes = value.to_vec(),
            _ => {}
        }
        Ok(())
    })?;
    Ok(request)
}

pub fn encode_test_app_upload_commit(value: &TestAppUploadCommitRequest, out: &mut Vec<u8>) {
    out.clear();
    put_varint_field(out, 1, value.upload_id);
}

pub fn decode_test_app_upload_commit(
    input: &[u8],
) -> Result<TestAppUploadCommitRequest, WireError> {
    let mut request = TestAppUploadCommitRequest::default();
    read_fields(input, |field, wire, value| {
        if field == 1 && wire == 0 {
            request.upload_id = read_varint_from(value)?;
        }
        Ok(())
    })?;
    Ok(request)
}

pub fn encode_test_app_launch(value: &TestAppLaunchRequest, out: &mut Vec<u8>) {
    out.clear();
    put_string(out, 1, &value.package_id);
    put_string(out, 2, &value.process_name);
    put_varint_field(out, 3, value.arg0);
}

pub fn decode_test_app_launch(input: &[u8]) -> Result<TestAppLaunchRequest, WireError> {
    let mut request = TestAppLaunchRequest::default();
    read_fields(input, |field, wire, value| {
        match (field, wire) {
            (1, 2) => request.package_id = read_string(value)?,
            (2, 2) => request.process_name = read_string(value)?,
            (3, 0) => request.arg0 = read_varint_from(value)?,
            _ => {}
        }
        Ok(())
    })?;
    Ok(request)
}

pub fn encode_app_bundle_upload_begin(value: &AppBundleUploadBeginRequest, out: &mut Vec<u8>) {
    out.clear();
    put_varint_field(out, 1, value.upload_id);
    put_varint_field(out, 2, value.archive_len);
}

pub fn decode_app_bundle_upload_begin(
    input: &[u8],
) -> Result<AppBundleUploadBeginRequest, WireError> {
    let mut request = AppBundleUploadBeginRequest::default();
    read_fields(input, |field, wire, value| {
        match (field, wire) {
            (1, 0) => request.upload_id = read_varint_from(value)?,
            (2, 0) => request.archive_len = read_varint_from(value)?,
            _ => {}
        }
        Ok(())
    })?;
    Ok(request)
}

pub fn encode_app_bundle_chunk(value: &AppBundleChunkRequest, out: &mut Vec<u8>) {
    out.clear();
    put_varint_field(out, 1, value.upload_id);
    put_varint_field(out, 2, value.offset);
    put_len_field(out, 3, &value.bytes);
}

pub fn decode_app_bundle_chunk(input: &[u8]) -> Result<AppBundleChunkRequest, WireError> {
    let mut request = AppBundleChunkRequest::default();
    read_fields(input, |field, wire, value| {
        match (field, wire) {
            (1, 0) => request.upload_id = read_varint_from(value)?,
            (2, 0) => request.offset = read_varint_from(value)?,
            (3, 2) => request.bytes = value.to_vec(),
            _ => {}
        }
        Ok(())
    })?;
    Ok(request)
}

pub fn encode_app_bundle_upload_commit(value: &AppBundleUploadCommitRequest, out: &mut Vec<u8>) {
    out.clear();
    put_varint_field(out, 1, value.upload_id);
}

pub fn decode_app_bundle_upload_commit(
    input: &[u8],
) -> Result<AppBundleUploadCommitRequest, WireError> {
    let mut request = AppBundleUploadCommitRequest::default();
    read_fields(input, |field, wire, value| {
        if field == 1 && wire == 0 {
            request.upload_id = read_varint_from(value)?;
        }
        Ok(())
    })?;
    Ok(request)
}

pub fn encode_app_uninstall(value: &AppUninstallRequest, out: &mut Vec<u8>) {
    out.clear();
    put_string(out, 1, &value.package_id);
}

pub fn decode_app_uninstall(input: &[u8]) -> Result<AppUninstallRequest, WireError> {
    let mut request = AppUninstallRequest::default();
    read_fields(input, |field, wire, value| {
        if field == 1 && wire == 2 {
            request.package_id = read_string(value)?;
        }
        Ok(())
    })?;
    Ok(request)
}

pub fn encode_app_launch(value: &AppLaunchRequest, out: &mut Vec<u8>) {
    out.clear();
    put_string(out, 1, &value.package_id);
    put_string(out, 2, &value.process_name);
    put_varint_field(out, 3, value.arg0);
    put_varint_field(out, 4, value.uid as u64);
}

pub fn decode_app_launch(input: &[u8]) -> Result<AppLaunchRequest, WireError> {
    let mut request = AppLaunchRequest::default();
    read_fields(input, |field, wire, value| {
        match (field, wire) {
            (1, 2) => request.package_id = read_string(value)?,
            (2, 2) => request.process_name = read_string(value)?,
            (3, 0) => request.arg0 = read_varint_from(value)?,
            (4, 0) => request.uid = read_varint_from(value)?,
            _ => {}
        }
        Ok(())
    })?;
    Ok(request)
}

pub fn encode_app_install_from_url(value: &AppInstallFromUrlRequest, out: &mut Vec<u8>) {
    out.clear();
    put_string(out, 1, &value.url);
}

pub fn decode_app_install_from_url(input: &[u8]) -> Result<AppInstallFromUrlRequest, WireError> {
    let mut request = AppInstallFromUrlRequest::default();
    read_fields(input, |field, wire, value| {
        if field == 1 && wire == 2 {
            request.url = read_string(value)?;
        }
        Ok(())
    })?;
    Ok(request)
}

pub fn encode_component_config_get(value: &ComponentConfigGetRequest, out: &mut Vec<u8>) {
    out.clear();
    put_string(out, 1, &value.package_id);
}

pub fn decode_component_config_get(input: &[u8]) -> Result<ComponentConfigGetRequest, WireError> {
    let mut request = ComponentConfigGetRequest::default();
    read_fields(input, |field, wire, value| {
        if field == 1 && wire == 2 {
            request.package_id = read_string(value)?;
        }
        Ok(())
    })?;
    Ok(request)
}

pub fn encode_component_config_get_response(value: &ComponentConfigGetResponse, out: &mut Vec<u8>) {
    out.clear();
    put_varint_field(out, 1, value.status as u64);
    put_varint_field(out, 2, value.generation);
    put_len_field(out, 3, &value.config);
}

pub fn decode_component_config_get_response(
    input: &[u8],
) -> Result<ComponentConfigGetResponse, WireError> {
    let mut response = ComponentConfigGetResponse::default();
    read_fields(input, |field, wire, value| {
        match (field, wire) {
            (1, 0) => response.status = read_varint_from(value)? as i32,
            (2, 0) => response.generation = read_varint_from(value)?,
            (3, 2) => response.config = value.to_vec(),
            _ => {}
        }
        Ok(())
    })?;
    Ok(response)
}

pub fn encode_component_config_set(value: &ComponentConfigSetRequest, out: &mut Vec<u8>) {
    out.clear();
    put_string(out, 1, &value.package_id);
    put_varint_field(out, 2, value.expected_generation);
    put_len_field(out, 3, &value.config);
}

pub fn decode_component_config_set(input: &[u8]) -> Result<ComponentConfigSetRequest, WireError> {
    let mut request = ComponentConfigSetRequest::default();
    read_fields(input, |field, wire, value| {
        match (field, wire) {
            (1, 2) => request.package_id = read_string(value)?,
            (2, 0) => request.expected_generation = read_varint_from(value)?,
            (3, 2) => request.config = value.to_vec(),
            _ => {}
        }
        Ok(())
    })?;
    Ok(request)
}

pub fn encode_component_config_reset(value: &ComponentConfigResetRequest, out: &mut Vec<u8>) {
    out.clear();
    put_string(out, 1, &value.package_id);
    put_varint_field(out, 2, value.expected_generation);
}

pub fn decode_component_config_reset(
    input: &[u8],
) -> Result<ComponentConfigResetRequest, WireError> {
    let mut request = ComponentConfigResetRequest::default();
    read_fields(input, |field, wire, value| {
        match (field, wire) {
            (1, 2) => request.package_id = read_string(value)?,
            (2, 0) => request.expected_generation = read_varint_from(value)?,
            _ => {}
        }
        Ok(())
    })?;
    Ok(request)
}

pub fn encode_component_config_mutation_response(
    value: &ComponentConfigMutationResponse,
    out: &mut Vec<u8>,
) {
    out.clear();
    put_varint_field(out, 1, value.status as u64);
    put_varint_field(out, 2, value.generation);
    put_string(out, 3, &value.message);
}

pub fn decode_component_config_mutation_response(
    input: &[u8],
) -> Result<ComponentConfigMutationResponse, WireError> {
    let mut response = ComponentConfigMutationResponse::default();
    read_fields(input, |field, wire, value| {
        match (field, wire) {
            (1, 0) => response.status = read_varint_from(value)? as i32,
            (2, 0) => response.generation = read_varint_from(value)?,
            (3, 2) => response.message = read_string(value)?,
            _ => {}
        }
        Ok(())
    })?;
    Ok(response)
}

pub fn encode_well_known_reload(value: &WellKnownReloadRequest, out: &mut Vec<u8>) {
    out.clear();
    put_string(out, 1, &value.domain);
}

pub fn decode_well_known_reload(input: &[u8]) -> Result<WellKnownReloadRequest, WireError> {
    let mut request = WellKnownReloadRequest::default();
    read_fields(input, |field, wire, value| {
        if field == 1 && wire == 2 {
            request.domain = read_string(value)?;
        }
        Ok(())
    })?;
    Ok(request)
}

pub fn encode_user_list_response(value: &UserListResponse, out: &mut Vec<u8>) {
    out.clear();
    put_varint_field(out, 1, value.status as u64);
    let mut item = Vec::new();
    for user in &value.users {
        item.clear();
        encode_user_info_fields(user, &mut item);
        put_len_field(out, 2, &item);
    }
}

pub fn decode_user_list_response(input: &[u8]) -> Result<UserListResponse, WireError> {
    let mut response = UserListResponse::default();
    read_fields(input, |field, wire, value| {
        match (field, wire) {
            (1, 0) => response.status = read_varint_from(value)? as i32,
            (2, 2) => response.users.push(decode_user_info(value)?),
            _ => {}
        }
        Ok(())
    })?;
    Ok(response)
}

pub fn encode_user_get_request(value: &UserGetRequest, out: &mut Vec<u8>) {
    out.clear();
    put_varint_field(out, 1, value.uid as u64);
}

pub fn decode_user_get_request(input: &[u8]) -> Result<UserGetRequest, WireError> {
    let mut request = UserGetRequest::default();
    read_fields(input, |field, wire, value| {
        if field == 1 && wire == 0 {
            request.uid = read_varint_from(value)?;
        }
        Ok(())
    })?;
    Ok(request)
}

pub fn encode_user_get_response(value: &UserGetResponse, out: &mut Vec<u8>) {
    out.clear();
    put_varint_field(out, 1, value.status as u64);
    let mut user = Vec::new();
    encode_user_info_fields(&value.user, &mut user);
    put_len_field(out, 2, &user);
}

pub fn decode_user_get_response(input: &[u8]) -> Result<UserGetResponse, WireError> {
    let mut response = UserGetResponse::default();
    read_fields(input, |field, wire, value| {
        match (field, wire) {
            (1, 0) => response.status = read_varint_from(value)? as i32,
            (2, 2) => response.user = decode_user_info(value)?,
            _ => {}
        }
        Ok(())
    })?;
    Ok(response)
}

pub fn encode_user_create_request(value: &UserCreateRequest, out: &mut Vec<u8>) {
    out.clear();
    put_varint_field(out, 1, value.uid as u64);
    put_string(out, 2, &value.name);
    put_string(out, 3, &value.display_name);
    put_string(out, 4, &value.password);
}

pub fn decode_user_create_request(input: &[u8]) -> Result<UserCreateRequest, WireError> {
    let mut request = UserCreateRequest::default();
    read_fields(input, |field, wire, value| {
        match (field, wire) {
            (1, 0) => request.uid = read_varint_from(value)?,
            (2, 2) => request.name = read_string(value)?,
            (3, 2) => request.display_name = read_string(value)?,
            (4, 2) => request.password = read_string(value)?,
            _ => {}
        }
        Ok(())
    })?;
    Ok(request)
}

pub fn encode_user_update_request(value: &UserUpdateRequest, out: &mut Vec<u8>) {
    out.clear();
    put_varint_field(out, 1, value.uid as u64);
    put_string(out, 2, &value.name);
    put_string(out, 3, &value.display_name);
    put_varint_field(out, 4, value.disabled as u64);
    put_string(out, 5, &value.current_password);
    put_string(out, 6, &value.new_password);
}

pub fn decode_user_update_request(input: &[u8]) -> Result<UserUpdateRequest, WireError> {
    let mut request = UserUpdateRequest::default();
    read_fields(input, |field, wire, value| {
        match (field, wire) {
            (1, 0) => request.uid = read_varint_from(value)?,
            (2, 2) => request.name = read_string(value)?,
            (3, 2) => request.display_name = read_string(value)?,
            (4, 0) => request.disabled = read_varint_from(value)? != 0,
            (5, 2) => request.current_password = read_string(value)?,
            (6, 2) => request.new_password = read_string(value)?,
            _ => {}
        }
        Ok(())
    })?;
    Ok(request)
}

pub fn encode_user_delete_request(value: &UserDeleteRequest, out: &mut Vec<u8>) {
    out.clear();
    put_varint_field(out, 1, value.uid as u64);
}

pub fn decode_user_delete_request(input: &[u8]) -> Result<UserDeleteRequest, WireError> {
    let mut request = UserDeleteRequest::default();
    read_fields(input, |field, wire, value| {
        if field == 1 && wire == 0 {
            request.uid = read_varint_from(value)?;
        }
        Ok(())
    })?;
    Ok(request)
}

pub fn encode_user_unlock_request(value: &UserUnlockRequest, out: &mut Vec<u8>) {
    out.clear();
    put_varint_field(out, 1, value.uid as u64);
    put_string(out, 2, &value.password);
}

pub fn decode_user_unlock_request(input: &[u8]) -> Result<UserUnlockRequest, WireError> {
    let mut request = UserUnlockRequest::default();
    read_fields(input, |field, wire, value| {
        match (field, wire) {
            (1, 0) => request.uid = read_varint_from(value)?,
            (2, 2) => request.password = read_string(value)?,
            _ => {}
        }
        Ok(())
    })?;
    Ok(request)
}

pub fn encode_user_lock_request(value: &UserLockRequest, out: &mut Vec<u8>) {
    out.clear();
    put_varint_field(out, 1, value.uid as u64);
}

pub fn decode_user_lock_request(input: &[u8]) -> Result<UserLockRequest, WireError> {
    let mut request = UserLockRequest::default();
    read_fields(input, |field, wire, value| {
        if field == 1 && wire == 0 {
            request.uid = read_varint_from(value)?;
        }
        Ok(())
    })?;
    Ok(request)
}

pub fn encode_tee_info_response(value: &TeeInfoResponse, out: &mut Vec<u8>) {
    out.clear();
    put_varint_field(out, 1, value.status as u64);
    put_varint_field(out, 2, value.present as u64);
    put_string(out, 3, &value.kind);
    put_varint_field(out, 4, value.secure_os_version as u64);
    put_varint_field(out, 5, value.anti_rollback_version as u64);
}

pub fn decode_tee_info_response(input: &[u8]) -> Result<TeeInfoResponse, WireError> {
    let mut response = TeeInfoResponse::default();
    read_fields(input, |field, wire, value| {
        match (field, wire) {
            (1, 0) => response.status = read_varint_from(value)? as i32,
            (2, 0) => response.present = read_varint_from(value)? != 0,
            (3, 2) => response.kind = read_string(value)?,
            (4, 0) => response.secure_os_version = read_varint_from(value)? as u32,
            (5, 0) => response.anti_rollback_version = read_varint_from(value)? as u32,
            _ => {}
        }
        Ok(())
    })?;
    Ok(response)
}

pub fn encode_tee_app_list_response(value: &TeeAppListResponse, out: &mut Vec<u8>) {
    out.clear();
    put_varint_field(out, 1, value.status as u64);
    let mut item = Vec::new();
    for app in &value.apps {
        item.clear();
        encode_tee_app_info_fields(app, &mut item);
        put_len_field(out, 2, &item);
    }
}

pub fn decode_tee_app_list_response(input: &[u8]) -> Result<TeeAppListResponse, WireError> {
    let mut response = TeeAppListResponse::default();
    read_fields(input, |field, wire, value| {
        match (field, wire) {
            (1, 0) => response.status = read_varint_from(value)? as i32,
            (2, 2) => response.apps.push(decode_tee_app_info(value)?),
            _ => {}
        }
        Ok(())
    })?;
    Ok(response)
}

pub fn encode_tee_install_app_request(value: &TeeInstallAppRequest, out: &mut Vec<u8>) {
    out.clear();
    put_len_field(out, 1, &value.payload);
}

pub fn decode_tee_install_app_request(input: &[u8]) -> Result<TeeInstallAppRequest, WireError> {
    let mut request = TeeInstallAppRequest::default();
    read_fields(input, |field, wire, value| {
        if field == 1 && wire == 2 {
            request.payload = value.to_vec();
        }
        Ok(())
    })?;
    Ok(request)
}

pub fn encode_tee_install_app_response(value: &TeeInstallAppResponse, out: &mut Vec<u8>) {
    out.clear();
    put_varint_field(out, 1, value.status as u64);
    put_len_field(out, 2, &value.uuid);
}

pub fn decode_tee_install_app_response(input: &[u8]) -> Result<TeeInstallAppResponse, WireError> {
    let mut response = TeeInstallAppResponse::default();
    read_fields(input, |field, wire, value| {
        match (field, wire) {
            (1, 0) => response.status = read_varint_from(value)? as i32,
            (2, 2) => response.uuid = value.to_vec(),
            _ => {}
        }
        Ok(())
    })?;
    Ok(response)
}

pub fn encode_tee_uuid_request(value: &TeeUuidRequest, out: &mut Vec<u8>) {
    out.clear();
    put_len_field(out, 1, &value.uuid);
}

pub fn decode_tee_uuid_request(input: &[u8]) -> Result<TeeUuidRequest, WireError> {
    let mut request = TeeUuidRequest::default();
    read_fields(input, |field, wire, value| {
        if field == 1 && wire == 2 {
            request.uuid = value.to_vec();
        }
        Ok(())
    })?;
    Ok(request)
}

pub fn encode_tee_session_request(value: &TeeSessionRequest, out: &mut Vec<u8>) {
    out.clear();
    put_varint_field(out, 1, value.session_id);
}

pub fn decode_tee_session_request(input: &[u8]) -> Result<TeeSessionRequest, WireError> {
    let mut request = TeeSessionRequest::default();
    read_fields(input, |field, wire, value| {
        if field == 1 && wire == 0 {
            request.session_id = read_varint_from(value)?;
        }
        Ok(())
    })?;
    Ok(request)
}

pub fn encode_tee_open_session_response(value: &TeeOpenSessionResponse, out: &mut Vec<u8>) {
    out.clear();
    put_varint_field(out, 1, value.status as u64);
    put_varint_field(out, 2, value.session_id);
}

pub fn decode_tee_open_session_response(input: &[u8]) -> Result<TeeOpenSessionResponse, WireError> {
    let mut response = TeeOpenSessionResponse::default();
    read_fields(input, |field, wire, value| {
        match (field, wire) {
            (1, 0) => response.status = read_varint_from(value)? as i32,
            (2, 0) => response.session_id = read_varint_from(value)?,
            _ => {}
        }
        Ok(())
    })?;
    Ok(response)
}

pub fn encode_tee_invoke_request(value: &TeeInvokeRequest, out: &mut Vec<u8>) {
    out.clear();
    put_varint_field(out, 1, value.session_id);
    put_varint_field(out, 2, value.command_id as u64);
    put_len_field(out, 3, &value.payload);
}

pub fn decode_tee_invoke_request(input: &[u8]) -> Result<TeeInvokeRequest, WireError> {
    let mut request = TeeInvokeRequest::default();
    read_fields(input, |field, wire, value| {
        match (field, wire) {
            (1, 0) => request.session_id = read_varint_from(value)?,
            (2, 0) => request.command_id = read_varint_from(value)? as u32,
            (3, 2) => request.payload = value.to_vec(),
            _ => {}
        }
        Ok(())
    })?;
    Ok(request)
}

pub fn encode_tee_invoke_response(value: &TeeInvokeResponse, out: &mut Vec<u8>) {
    out.clear();
    put_varint_field(out, 1, value.status as u64);
    put_len_field(out, 2, &value.response);
}

pub fn decode_tee_invoke_response(input: &[u8]) -> Result<TeeInvokeResponse, WireError> {
    let mut response = TeeInvokeResponse::default();
    read_fields(input, |field, wire, value| {
        match (field, wire) {
            (1, 0) => response.status = read_varint_from(value)? as i32,
            (2, 2) => response.response = value.to_vec(),
            _ => {}
        }
        Ok(())
    })?;
    Ok(response)
}

pub fn encode_tee_update_core_request(value: &TeeUpdateCoreRequest, out: &mut Vec<u8>) {
    out.clear();
    put_varint_field(out, 1, value.generation);
    put_string(out, 2, &value.target);
    put_len_field(out, 3, &value.artifact_hash);
    put_len_field(out, 4, &value.image);
}

pub fn decode_tee_update_core_request(input: &[u8]) -> Result<TeeUpdateCoreRequest, WireError> {
    let mut request = TeeUpdateCoreRequest::default();
    read_fields(input, |field, wire, value| {
        match (field, wire) {
            (1, 0) => request.generation = read_varint_from(value)?,
            (2, 2) => request.target = read_string(value)?,
            (3, 2) => request.artifact_hash = value.to_vec(),
            (4, 2) => request.image = value.to_vec(),
            _ => {}
        }
        Ok(())
    })?;
    Ok(request)
}

pub fn encode_tee_update_core_response(value: &TeeUpdateCoreResponse, out: &mut Vec<u8>) {
    out.clear();
    put_varint_field(out, 1, value.status as u64);
    put_varint_field(out, 2, value.new_version as u64);
    put_string(out, 3, &value.message);
}

pub fn decode_tee_update_core_response(input: &[u8]) -> Result<TeeUpdateCoreResponse, WireError> {
    let mut response = TeeUpdateCoreResponse::default();
    read_fields(input, |field, wire, value| {
        match (field, wire) {
            (1, 0) => response.status = read_varint_from(value)? as i32,
            (2, 0) => response.new_version = read_varint_from(value)? as u32,
            (3, 2) => response.message = read_string(value)?,
            _ => {}
        }
        Ok(())
    })?;
    Ok(response)
}

pub fn encode_tee_update_status_response(value: &TeeUpdateStatusResponse, out: &mut Vec<u8>) {
    out.clear();
    put_varint_field(out, 1, value.status as u64);
    put_string(out, 2, &value.update_status);
    put_varint_field(out, 3, value.generation);
    put_string(out, 4, &value.message);
    put_string(out, 5, &value.phase);
    put_string(out, 6, &value.active_slot);
    put_string(out, 7, &value.pending_slot);
    put_varint_field(out, 8, value.reboot_required as u64);
    put_varint_field(out, 9, value.rollback_available as u64);
}

pub fn decode_tee_update_status_response(
    input: &[u8],
) -> Result<TeeUpdateStatusResponse, WireError> {
    let mut response = TeeUpdateStatusResponse::default();
    read_fields(input, |field, wire, value| {
        match (field, wire) {
            (1, 0) => response.status = read_varint_from(value)? as i32,
            (2, 2) => response.update_status = read_string(value)?,
            (3, 0) => response.generation = read_varint_from(value)?,
            (4, 2) => response.message = read_string(value)?,
            (5, 2) => response.phase = read_string(value)?,
            (6, 2) => response.active_slot = read_string(value)?,
            (7, 2) => response.pending_slot = read_string(value)?,
            (8, 0) => response.reboot_required = read_varint_from(value)? != 0,
            (9, 0) => response.rollback_available = read_varint_from(value)? != 0,
            _ => {}
        }
        Ok(())
    })?;
    Ok(response)
}

pub fn encode_trace_start_request(value: &TraceStartRequest, out: &mut Vec<u8>) {
    out.clear();
    put_varint_field(out, 1, value.categories as u64);
    put_varint_field(out, 2, value.buffer_mode as u64);
    put_varint_field(out, 3, value.buffer_size_kb as u64);
    put_varint_field(out, 4, value.output_format as u64);
}

pub fn decode_trace_start_request(input: &[u8]) -> Result<TraceStartRequest, WireError> {
    let mut request = TraceStartRequest::default();
    read_fields(input, |field, wire, value| {
        match (field, wire) {
            (1, 0) => request.categories = read_varint_from(value)? as u32,
            (2, 0) => request.buffer_mode = read_varint_from(value)? as u32,
            (3, 0) => request.buffer_size_kb = read_varint_from(value)? as u32,
            (4, 0) => request.output_format = read_varint_from(value)? as u32,
            _ => {}
        }
        Ok(())
    })?;
    if request.output_format == 0 {
        request.output_format = 1;
    }
    Ok(request)
}

pub fn encode_trace_status_response(value: &TraceStatusResponse, out: &mut Vec<u8>) {
    out.clear();
    put_varint_field(out, 1, value.status as u64);
    put_varint_field(out, 2, value.state as u64);
    put_varint_field(out, 3, value.categories as u64);
    put_varint_field(out, 4, value.buffer_mode as u64);
    put_varint_field(out, 5, value.buffer_size_kb as u64);
    put_varint_field(out, 6, value.output_format as u64);
    put_varint_field(out, 7, value.producer_count as u64);
    put_varint_field(out, 8, value.event_count);
    put_varint_field(out, 9, value.dropped_count);
}

pub fn decode_trace_status_response(input: &[u8]) -> Result<TraceStatusResponse, WireError> {
    let mut response = TraceStatusResponse::default();
    read_fields(input, |field, wire, value| {
        match (field, wire) {
            (1, 0) => response.status = read_varint_from(value)? as i32,
            (2, 0) => response.state = read_varint_from(value)? as u32,
            (3, 0) => response.categories = read_varint_from(value)? as u32,
            (4, 0) => response.buffer_mode = read_varint_from(value)? as u32,
            (5, 0) => response.buffer_size_kb = read_varint_from(value)? as u32,
            (6, 0) => response.output_format = read_varint_from(value)? as u32,
            (7, 0) => response.producer_count = read_varint_from(value)? as u32,
            (8, 0) => response.event_count = read_varint_from(value)?,
            (9, 0) => response.dropped_count = read_varint_from(value)?,
            _ => {}
        }
        Ok(())
    })?;
    if response.output_format == 0 {
        response.output_format = 1;
    }
    Ok(response)
}

pub fn encode_trace_stop_request(value: &TraceStopRequest, out: &mut Vec<u8>) {
    out.clear();
    put_varint_field(out, 1, value.offset);
    put_varint_field(out, 2, value.max_bytes as u64);
}

pub fn decode_trace_stop_request(input: &[u8]) -> Result<TraceStopRequest, WireError> {
    let mut request = TraceStopRequest::default();
    read_fields(input, |field, wire, value| {
        match (field, wire) {
            (1, 0) => request.offset = read_varint_from(value)?,
            (2, 0) => request.max_bytes = read_varint_from(value)? as u32,
            _ => {}
        }
        Ok(())
    })?;
    Ok(request)
}

pub fn encode_trace_stop_response(value: &TraceStopResponse, out: &mut Vec<u8>) {
    out.clear();
    put_varint_field(out, 1, value.status as u64);
    put_varint_field(out, 2, value.offset);
    put_varint_field(out, 3, value.total_len);
    put_len_field(out, 4, &value.bytes);
    put_varint_field(out, 5, value.complete as u64);
    put_varint_field(out, 6, value.output_format as u64);
}

pub fn decode_trace_stop_response(input: &[u8]) -> Result<TraceStopResponse, WireError> {
    let mut response = TraceStopResponse::default();
    read_fields(input, |field, wire, value| {
        match (field, wire) {
            (1, 0) => response.status = read_varint_from(value)? as i32,
            (2, 0) => response.offset = read_varint_from(value)?,
            (3, 0) => response.total_len = read_varint_from(value)?,
            (4, 2) => response.bytes = value.to_vec(),
            (5, 0) => response.complete = read_varint_from(value)? != 0,
            (6, 0) => response.output_format = read_varint_from(value)? as u32,
            _ => {}
        }
        Ok(())
    })?;
    if response.output_format == 0 {
        response.output_format = 1;
    }
    Ok(response)
}

pub fn encode_update_upload_begin(value: &UpdateUploadBeginRequest, out: &mut Vec<u8>) {
    out.clear();
    put_varint_field(out, 1, value.upload_id);
    put_varint_field(out, 2, value.manifest_len);
    put_varint_field(out, 3, value.artifact_len);
}

pub fn decode_update_upload_begin(input: &[u8]) -> Result<UpdateUploadBeginRequest, WireError> {
    let mut request = UpdateUploadBeginRequest::default();
    read_fields(input, |field, wire, value| {
        match (field, wire) {
            (1, 0) => request.upload_id = read_varint_from(value)?,
            (2, 0) => request.manifest_len = read_varint_from(value)?,
            (3, 0) => request.artifact_len = read_varint_from(value)?,
            _ => {}
        }
        Ok(())
    })?;
    Ok(request)
}

pub fn encode_update_chunk(value: &UpdateChunkRequest, out: &mut Vec<u8>) {
    out.clear();
    put_varint_field(out, 1, value.upload_id);
    put_varint_field(out, 2, value.stream as u64);
    put_varint_field(out, 3, value.offset);
    put_len_field(out, 4, &value.bytes);
}

pub fn decode_update_chunk(input: &[u8]) -> Result<UpdateChunkRequest, WireError> {
    let mut request = UpdateChunkRequest::default();
    read_fields(input, |field, wire, value| {
        match (field, wire) {
            (1, 0) => request.upload_id = read_varint_from(value)?,
            (2, 0) => request.stream = read_varint_from(value)? as u32,
            (3, 0) => request.offset = read_varint_from(value)?,
            (4, 2) => request.bytes = value.to_vec(),
            _ => {}
        }
        Ok(())
    })?;
    Ok(request)
}

pub fn encode_update_upload_commit(value: &UpdateUploadCommitRequest, out: &mut Vec<u8>) {
    out.clear();
    put_varint_field(out, 1, value.upload_id);
}

pub fn decode_update_upload_commit(input: &[u8]) -> Result<UpdateUploadCommitRequest, WireError> {
    let mut request = UpdateUploadCommitRequest::default();
    read_fields(input, |field, wire, value| {
        if field == 1 && wire == 0 {
            request.upload_id = read_varint_from(value)?;
        }
        Ok(())
    })?;
    Ok(request)
}

pub fn encode_update_check_request(value: &UpdateCheckRequest, out: &mut Vec<u8>) {
    out.clear();
    put_varint_field(out, 1, value.selector_kind as u64);
    put_string(out, 2, &value.target);
    put_varint_field(out, 3, value.all as u64);
    put_varint_field(out, 4, value.stage as u64);
    put_varint_field(out, 5, value.apply as u64);
}

pub fn decode_update_check_request(input: &[u8]) -> Result<UpdateCheckRequest, WireError> {
    let mut request = UpdateCheckRequest::default();
    read_fields(input, |field, wire, value| {
        match (field, wire) {
            (1, 0) => request.selector_kind = read_varint_from(value)? as u32,
            (2, 2) => request.target = read_string(value)?,
            (3, 0) => request.all = read_varint_from(value)? != 0,
            (4, 0) => request.stage = read_varint_from(value)? != 0,
            (5, 0) => request.apply = read_varint_from(value)? != 0,
            _ => {}
        }
        Ok(())
    })?;
    Ok(request)
}

pub fn encode_update_check_response(value: &UpdateCheckResponse, out: &mut Vec<u8>) {
    out.clear();
    put_varint_field(out, 1, value.status as u64);
    put_string(out, 2, &value.message);
    let mut item = Vec::new();
    for candidate in &value.candidates {
        item.clear();
        encode_update_candidate_info(candidate, &mut item);
        put_len_field(out, 3, &item);
    }
}

pub fn decode_update_check_response(input: &[u8]) -> Result<UpdateCheckResponse, WireError> {
    let mut response = UpdateCheckResponse::default();
    read_fields(input, |field, wire, value| {
        match (field, wire) {
            (1, 0) => response.status = read_varint_from(value)? as i32,
            (2, 2) => response.message = read_string(value)?,
            (3, 2) => response
                .candidates
                .push(decode_update_candidate_info(value)?),
            _ => {}
        }
        Ok(())
    })?;
    Ok(response)
}

fn encode_update_candidate_info(value: &UpdateCandidateInfo, out: &mut Vec<u8>) {
    put_string(out, 1, &value.target);
    put_string(out, 2, &value.path);
    put_string(out, 3, &value.url);
    put_varint_field(out, 4, value.kind as u64);
    put_varint_field(out, 5, value.generation);
    put_varint_field(out, 6, value.length);
}

fn decode_update_candidate_info(input: &[u8]) -> Result<UpdateCandidateInfo, WireError> {
    let mut candidate = UpdateCandidateInfo::default();
    read_fields(input, |field, wire, value| {
        match (field, wire) {
            (1, 2) => candidate.target = read_string(value)?,
            (2, 2) => candidate.path = read_string(value)?,
            (3, 2) => candidate.url = read_string(value)?,
            (4, 0) => candidate.kind = read_varint_from(value)? as u32,
            (5, 0) => candidate.generation = read_varint_from(value)?,
            (6, 0) => candidate.length = read_varint_from(value)?,
            _ => {}
        }
        Ok(())
    })?;
    Ok(candidate)
}

pub fn encode_debug_status(value: &DebugStatusResponse, out: &mut Vec<u8>) {
    out.clear();
    put_varint_field(out, 1, value.status as u64);
    put_string(out, 2, &value.message);
}

pub fn decode_debug_status(input: &[u8]) -> Result<DebugStatusResponse, WireError> {
    let mut response = DebugStatusResponse::default();
    read_fields(input, |field, wire, value| {
        match (field, wire) {
            (1, 0) => response.status = read_varint_from(value)? as i32,
            (2, 2) => response.message = read_string(value)?,
            _ => {}
        }
        Ok(())
    })?;
    Ok(response)
}

fn decode_process(input: &[u8]) -> Result<ProcessInfo, WireError> {
    let mut process = ProcessInfo {
        pid: 0,
        name: String::new(),
        state: String::new(),
        package_id: String::new(),
        ..Default::default()
    };
    read_fields(input, |field, wire, value| {
        match (field, wire) {
            (1, 0) => process.pid = read_varint_from(value)?,
            (2, 2) => process.name = read_string(value)?,
            (3, 2) => process.state = read_string(value)?,
            (4, 2) => process.package_id = read_string(value)?,
            (5, 0) => process.main_thread_id = read_varint_from(value)?,
            (6, 0) => {
                process.resource_group_id = Some(
                    u32::try_from(read_varint_from(value)?).map_err(|_| WireError::InvalidProto)?,
                )
            }
            (7, 2) => process.resource_group_name = read_string(value)?,
            (8, 0) => {
                process.parent_resource_group_id = Some(
                    u32::try_from(read_varint_from(value)?).map_err(|_| WireError::InvalidProto)?,
                )
            }
            (9, 2) => process.parent_resource_group_name = read_string(value)?,
            _ => {}
        }
        Ok(())
    })?;
    Ok(process)
}

fn decode_app(input: &[u8]) -> Result<AppInfo, WireError> {
    let mut app = AppInfo {
        package_id: String::new(),
        name: String::new(),
        state: String::new(),
        source: String::new(),
        protected: false,
    };
    read_fields(input, |field, wire, value| {
        match (field, wire) {
            (1, 2) => app.package_id = read_string(value)?,
            (2, 2) => app.name = read_string(value)?,
            (3, 2) => app.state = read_string(value)?,
            (4, 2) => app.source = read_string(value)?,
            (5, 0) => app.protected = read_varint_from(value)? != 0,
            _ => {}
        }
        Ok(())
    })?;
    Ok(app)
}

fn encode_user_info_fields(user: &UserInfo, out: &mut Vec<u8>) {
    put_varint_field(out, 1, user.uid as u64);
    put_string(out, 2, &user.name);
    put_string(out, 3, &user.display_name);
    put_varint_field(out, 4, user.disabled as u64);
    put_string(out, 5, &user.home_path);
    put_varint_field(out, 6, user.unlocked as u64);
}

fn encode_tee_app_info_fields(app: &TeeAppInfo, out: &mut Vec<u8>) {
    put_len_field(out, 1, &app.uuid);
    put_varint_field(out, 2, app.version as u64);
    put_varint_field(out, 3, app.active_sessions as u64);
    put_string(out, 4, &app.entry_point_name);
    put_varint_field(out, 5, app.storage_bytes_used);
    put_string(out, 6, &app.package_id);
    put_varint_field(out, 7, app.protected as u64);
    put_varint_field(out, 8, app.package_managed as u64);
}

fn decode_tee_app_info(input: &[u8]) -> Result<TeeAppInfo, WireError> {
    let mut app = TeeAppInfo::default();
    read_fields(input, |field, wire, value| {
        match (field, wire) {
            (1, 2) => app.uuid = value.to_vec(),
            (2, 0) => app.version = read_varint_from(value)? as u32,
            (3, 0) => app.active_sessions = read_varint_from(value)? as u32,
            (4, 2) => app.entry_point_name = read_string(value)?,
            (5, 0) => app.storage_bytes_used = read_varint_from(value)?,
            (6, 2) => app.package_id = read_string(value)?,
            (7, 0) => app.protected = read_varint_from(value)? != 0,
            (8, 0) => app.package_managed = read_varint_from(value)? != 0,
            _ => {}
        }
        Ok(())
    })?;
    Ok(app)
}

fn decode_user_info(input: &[u8]) -> Result<UserInfo, WireError> {
    let mut user = UserInfo::default();
    read_fields(input, |field, wire, value| {
        match (field, wire) {
            (1, 0) => user.uid = read_varint_from(value)?,
            (2, 2) => user.name = read_string(value)?,
            (3, 2) => user.display_name = read_string(value)?,
            (4, 0) => user.disabled = read_varint_from(value)? != 0,
            (5, 2) => user.home_path = read_string(value)?,
            (6, 0) => user.unlocked = read_varint_from(value)? != 0,
            _ => {}
        }
        Ok(())
    })?;
    Ok(user)
}

fn read_fields<F>(mut input: &[u8], mut f: F) -> Result<(), WireError>
where
    F: FnMut(u32, u8, &[u8]) -> Result<(), WireError>,
{
    while !input.is_empty() {
        let (key, used) = read_varint(input)?;
        input = &input[used..];
        if key >> 3 == 0 || key >> 3 > 0x1fff_ffff {
            return Err(WireError::InvalidProto);
        }
        let field = (key >> 3) as u32;
        let wire = (key & 7) as u8;
        match wire {
            0 => {
                let (_, used) = read_varint(input)?;
                f(field, wire, &input[..used])?;
                input = &input[used..];
            }
            2 => {
                let (len, used) = read_varint(input)?;
                input = &input[used..];
                let len = len as usize;
                if len > input.len() {
                    return Err(WireError::InvalidProto);
                }
                f(field, wire, &input[..len])?;
                input = &input[len..];
            }
            1 | 5 => {
                let len = if wire == 1 { 8 } else { 4 };
                let bytes = input.get(..len).ok_or(WireError::InvalidProto)?;
                f(field, wire, bytes)?;
                input = &input[len..];
            }
            _ => return Err(WireError::InvalidProto),
        }
    }
    Ok(())
}

fn put_string(out: &mut Vec<u8>, field: u32, value: &str) {
    put_len_field(out, field, value.as_bytes());
}

fn put_len_field(out: &mut Vec<u8>, field: u32, value: &[u8]) {
    put_varint(out, ((field as u64) << 3) | 2);
    put_varint(out, value.len() as u64);
    out.extend_from_slice(value);
}

fn put_varint_field(out: &mut Vec<u8>, field: u32, value: u64) {
    put_varint(out, (field as u64) << 3);
    put_varint(out, value);
}

fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn read_string(input: &[u8]) -> Result<String, WireError> {
    core::str::from_utf8(input)
        .map(str::to_string)
        .map_err(|_| WireError::Utf8)
}

fn read_varint_from(input: &[u8]) -> Result<u64, WireError> {
    read_varint(input).map(|(value, _)| value)
}

fn read_varint(input: &[u8]) -> Result<(u64, usize), WireError> {
    let mut value = 0u64;
    for (index, byte) in input.iter().copied().enumerate().take(10) {
        if index == 9 && byte > 1 {
            return Err(WireError::InvalidProto);
        }
        value |= ((byte & 0x7f) as u64) << (index * 7);
        if byte & 0x80 == 0 {
            return Ok((value, index + 1));
        }
    }
    Err(WireError::InvalidProto)
}

mod shell;
pub use shell::*;
