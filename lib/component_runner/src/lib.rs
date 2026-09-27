use component_runner_fidl::{
    ComponentControllerConnectRequest, ComponentControllerKillRequest,
    ComponentControllerSendSignalRequest, ComponentControllerStopRequest,
    ComponentRunnerEventsOnReadyRequest, ComponentRunnerEventsOnStopRequest,
    ComponentRunnerStartRequest, FidlDecode, FidlEncode, HandleRef,
};
use kernel_fidl::Status;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

static EVENTS: AtomicU64 = AtomicU64::new(0);
static CONTROLLER: AtomicU64 = AtomicU64::new(0);
static READY_SENT: AtomicBool = AtomicBool::new(false);
static STOP_SENT: AtomicBool = AtomicBool::new(false);
static STOP_REQUESTED: AtomicBool = AtomicBool::new(false);

pub const ELF_PROGRAM_TYPE_URL: &str = "type.googleapis.com/bexos.app.ELFRunnerOptions";

#[derive(Debug, Eq, PartialEq)]
pub enum Error {
    Channel,
    Envelope,
    Protocol,
    Runner,
    Program,
    Payloads,
    Package,
}

/// Persistent lifecycle state used by appd when validating the callback
/// protocol. A terminal callback is valid before readiness (failed Start), but
/// readiness after a terminal callback and duplicate callbacks are violations.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EventState {
    pub ready: bool,
    pub stopped: bool,
}

impl EventState {
    pub const fn new(ready: bool, stopped: bool) -> Self {
        Self { ready, stopped }
    }

    pub fn on_ready(&mut self, status: Status) -> Result<(), Error> {
        if status != Status::Ok || self.ready || self.stopped {
            return Err(Error::Protocol);
        }
        self.ready = true;
        Ok(())
    }

    pub fn on_stop(&mut self) -> Result<(), Error> {
        if self.stopped {
            return Err(Error::Protocol);
        }
        self.stopped = true;
        Ok(())
    }
}

pub struct Start {
    pub resolved_url: String,
    pub program: Vec<u8>,
    pub package_dir: Option<u64>,
    pub dependencies: Vec<ResolvedDependency>,
    pub startup: bexos_userspace::Channel,
    pub job: u64,
    pub service: bool,
    pub migratable: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DependencyKind {
    Native,
    WasmComponent,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DependencyReference {
    pub package_name: String,
    pub abi_version: u32,
    pub soname: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedDependency {
    pub package_name: String,
    pub mount_alias: String,
    pub export_name: String,
    pub export_path: String,
    pub symbol_prefix: String,
    pub soname: String,
    pub abi_version: u32,
    pub kind: DependencyKind,
    pub direct_dependencies: Vec<DependencyReference>,
    pub directory: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ControllerAction {
    Stop,
    Kill,
    Signal(u32),
    Connect(LateServiceConnection),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LateServiceConnection {
    pub service: String,
    pub protocol: String,
    pub capability: String,
    pub method_ordinals: Vec<u64>,
    pub permission_values: Vec<String>,
    pub caller_package: String,
    pub caller_uid: u64,
    pub caller_foreground: bool,
    pub endpoint: u64,
}

pub fn receive_start(
    channel: bexos_userspace::Channel,
    expected_runner: &str,
    expected_program_type: &str,
    require_package_directory: bool,
) -> Result<Start, Error> {
    let message = channel.recv_blocking().map_err(|_| Error::Channel)?;
    if message.bytes.len() < 8
        || u64::from_le_bytes(message.bytes[..8].try_into().map_err(|_| Error::Envelope)?) != 1
    {
        close_all(&message.handles);
        return Err(Error::Envelope);
    }
    let handles = message
        .handles
        .iter()
        .map(|raw| HandleRef { raw: *raw })
        .collect::<Vec<_>>();
    let request = ComponentRunnerStartRequest::decode(&message.bytes[8..], &handles)
        .map_err(|_| Error::Envelope)?;
    EVENTS.store(request.events.raw, Ordering::Release);
    CONTROLLER.store(request.controller.raw, Ordering::Release);
    READY_SENT.store(false, Ordering::Release);
    STOP_SENT.store(false, Ordering::Release);
    STOP_REQUESTED.store(false, Ordering::Release);
    if request.start_info.resolved_url.len() > 256
        || request.start_info.runner.len() > 32
        || request.start_info.program.type_url.len() > 256
        || request.start_info.program.payload.len() > 65_536
        || request.start_info.package_dir.len() > 1
        || request.start_info.dependencies.len() > 64
    {
        close_all_except(&message.handles, request.events.raw);
        return Err(Error::Envelope);
    }
    if request.start_info.runner != expected_runner {
        close_all_except(&message.handles, request.events.raw);
        return Err(Error::Runner);
    }
    if request.start_info.program.type_url != expected_program_type
        || request.start_info.program.payload.is_empty()
    {
        close_all_except(&message.handles, request.events.raw);
        return Err(Error::Program);
    }
    if require_package_directory && request.start_info.package_dir.is_empty() {
        close_all_except(&message.handles, request.events.raw);
        return Err(Error::Payloads);
    }
    let mut dependencies = Vec::with_capacity(request.start_info.dependencies.len());
    for index in 0..request.start_info.dependencies.len() {
        let dependency = request
            .start_info
            .dependencies
            .get(index)
            .map_err(|_| Error::Envelope)?;
        if dependency.package_name.len() > 128
            || dependency.mount_alias.len() > 64
            || dependency.export_name.len() > 128
            || dependency.export_path.len() > 256
            || dependency.symbol_prefix.len() > 64
            || dependency.soname.len() > 128
            || dependency.direct_dependencies.len() > 64
        {
            close_all_except(&message.handles, request.events.raw);
            return Err(Error::Envelope);
        }
        let mut direct_dependencies = Vec::with_capacity(dependency.direct_dependencies.len());
        for direct_index in 0..dependency.direct_dependencies.len() {
            let direct = dependency
                .direct_dependencies
                .get(direct_index)
                .map_err(|_| Error::Envelope)?;
            if direct.package_name.len() > 128 || direct.soname.len() > 128 {
                close_all_except(&message.handles, request.events.raw);
                return Err(Error::Envelope);
            }
            direct_dependencies.push(DependencyReference {
                package_name: direct.package_name.into(),
                abi_version: direct.abi_version,
                soname: direct.soname.into(),
            });
        }
        dependencies.push(ResolvedDependency {
            package_name: dependency.package_name.into(),
            mount_alias: dependency.mount_alias.into(),
            export_name: dependency.export_name.into(),
            export_path: dependency.export_path.into(),
            symbol_prefix: dependency.symbol_prefix.into(),
            soname: dependency.soname.into(),
            abi_version: dependency.abi_version,
            kind: match dependency.kind {
                component_runner_fidl::DependencyKind::Native => DependencyKind::Native,
                component_runner_fidl::DependencyKind::WasmComponent => {
                    DependencyKind::WasmComponent
                }
            },
            direct_dependencies,
            directory: dependency.directory.raw,
        });
    }
    Ok(Start {
        resolved_url: request.start_info.resolved_url.into(),
        program: request.start_info.program.payload.to_vec(),
        package_dir: request
            .start_info
            .package_dir
            .get(0)
            .map(|handle| handle.raw),
        dependencies,
        startup: bexos_userspace::Channel(request.start_info.startup.raw),
        job: request.start_info.job.raw,
        service: request.start_info.service,
        migratable: request.start_info.migratable,
    })
}

pub fn ready() -> Result<(), Error> {
    if READY_SENT.swap(true, Ordering::AcqRel) {
        return Ok(());
    }
    let channel = EVENTS.load(Ordering::Acquire);
    if channel == 0 {
        return Ok(());
    }
    send_oneway(
        channel,
        1,
        &ComponentRunnerEventsOnReadyRequest { status: Status::Ok },
    )
}

pub fn stop(status: Status, exit_code: i64) -> Result<(), Error> {
    if STOP_SENT.swap(true, Ordering::AcqRel) {
        return Ok(());
    }
    let channel = EVENTS.swap(0, Ordering::AcqRel);
    if channel == 0 {
        return Ok(());
    }
    let result = send_oneway(
        channel,
        2,
        &ComponentRunnerEventsOnStopRequest { status, exit_code },
    );
    let _ = bexos_userspace::Memory::close(channel);
    result
}

pub fn stop_requested() -> bool {
    STOP_REQUESTED.load(Ordering::Acquire)
}

/// Reads one immutable package-relative file through a supplied directory
/// capability. Paths are canonical `/pkg/...` paths and traversal is rejected.
pub fn read_package_file(directory: u64, path: &str, max_bytes: u64) -> Result<Vec<u8>, Error> {
    let relative = path.strip_prefix("/pkg/").ok_or(Error::Package)?;
    if relative.is_empty()
        || relative.len() > 256
        || relative.contains('\\')
        || relative
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return Err(Error::Package);
    }
    let file = bexos_userspace::fs::open(bexos_userspace::Channel(directory), relative, 1)
        .map_err(|_| Error::Package)?;
    let result = (|| {
        let attributes = bexos_userspace::fs::attributes(file).map_err(|_| Error::Package)?;
        if attributes.size_bytes == 0 || attributes.size_bytes > max_bytes {
            return Err(Error::Package);
        }
        let bytes =
            bexos_userspace::fs::read(file, attributes.size_bytes).map_err(|_| Error::Package)?;
        if bytes.len() as u64 != attributes.size_bytes {
            return Err(Error::Package);
        }
        Ok(bytes)
    })();
    let _ = bexos_userspace::Memory::close(file.0);
    result
}

pub fn close_start_directories(start: &Start) {
    if let Some(package_dir) = start.package_dir {
        let _ = bexos_userspace::Memory::close(package_dir);
    }
    for dependency in &start.dependencies {
        let _ = bexos_userspace::Memory::close(dependency.directory);
    }
}

pub fn request_stop(controller: u64) -> Result<(), Error> {
    send_oneway(controller, 1, &ComponentControllerStopRequest {})
}

pub fn request_kill(controller: u64) -> Result<(), Error> {
    send_oneway(controller, 2, &ComponentControllerKillRequest {})
}

pub fn send_signal(controller: u64, signal: u32) -> Result<(), Error> {
    send_oneway(
        controller,
        3,
        &ComponentControllerSendSignalRequest { signal },
    )
}

pub fn poll_controller() -> Result<Option<ControllerAction>, Error> {
    let channel = CONTROLLER.load(Ordering::Acquire);
    if channel == 0 {
        return Ok(None);
    }
    let message = match bexos_userspace::Channel(channel).recv() {
        Ok(message) => message,
        Err(kernel_fidl::Status::ErrTimedOut) => return Ok(None),
        Err(kernel_fidl::Status::ErrPeerClosed) => {
            STOP_REQUESTED.store(true, Ordering::Release);
            return Ok(Some(ControllerAction::Kill));
        }
        Err(_) => return Err(Error::Channel),
    };
    let handles = message
        .handles
        .iter()
        .map(|raw| HandleRef { raw: *raw })
        .collect::<Vec<_>>();
    let action = match decode_controller_action(&message.bytes, &handles) {
        Ok(action) => action,
        Err(error) => {
            close_all(&message.handles);
            return Err(error);
        }
    };
    if matches!(action, ControllerAction::Stop | ControllerAction::Kill) {
        STOP_REQUESTED.store(true, Ordering::Release);
    }
    Ok(Some(action))
}

fn decode_controller_action(
    bytes: &[u8],
    handles: &[HandleRef],
) -> Result<ControllerAction, Error> {
    if bytes.len() < 8 {
        return Err(Error::Envelope);
    }
    let ordinal = u64::from_le_bytes(bytes[..8].try_into().map_err(|_| Error::Envelope)?);
    let action = match ordinal {
        1 => {
            ComponentControllerStopRequest::decode(&bytes[8..], handles)
                .map_err(|_| Error::Envelope)?;
            ControllerAction::Stop
        }
        2 => {
            ComponentControllerKillRequest::decode(&bytes[8..], handles)
                .map_err(|_| Error::Envelope)?;
            ControllerAction::Kill
        }
        3 => ControllerAction::Signal(
            ComponentControllerSendSignalRequest::decode(&bytes[8..], handles)
                .map_err(|_| Error::Envelope)?
                .signal,
        ),
        4 => {
            let request = ComponentControllerConnectRequest::decode(&bytes[8..], handles)
                .map_err(|_| Error::Envelope)?;
            let connection = request.connection;
            if connection.service.len() > 128
                || connection.protocol.len() > 128
                || connection.capability.len() > 128
                || connection.method_ordinals.len() > 512
                || connection.method_ordinals.len() % 8 != 0
                || connection.permission_values.len() > 16
                || connection.caller_package.len() > 128
            {
                return Err(Error::Envelope);
            }
            let method_ordinals = connection
                .method_ordinals
                .chunks_exact(8)
                .map(|bytes| u64::from_le_bytes(bytes.try_into().unwrap()))
                .collect();
            let mut permission_values = Vec::with_capacity(connection.permission_values.len());
            for index in 0..connection.permission_values.len() {
                let value = connection
                    .permission_values
                    .get(index)
                    .map_err(|_| Error::Envelope)?;
                if value.len() > 128 {
                    return Err(Error::Envelope);
                }
                permission_values.push(value.to_string());
            }
            ControllerAction::Connect(LateServiceConnection {
                service: connection.service.into(),
                protocol: connection.protocol.into(),
                capability: connection.capability.into(),
                method_ordinals,
                permission_values,
                caller_package: connection.caller_package.into(),
                caller_uid: connection.caller_uid,
                caller_foreground: connection.caller_foreground,
                endpoint: connection.endpoint.raw,
            })
        }
        _ => return Err(Error::Envelope),
    };
    Ok(action)
}

fn close_all(handles: &[u64]) {
    for handle in handles {
        let _ = bexos_userspace::Memory::close(*handle);
    }
}

fn close_all_except(handles: &[u64], retained: u64) {
    for handle in handles.iter().copied().filter(|handle| *handle != retained) {
        let _ = bexos_userspace::Memory::close(handle);
    }
}

fn send_oneway<T: FidlEncode>(channel: u64, ordinal: u64, value: &T) -> Result<(), Error> {
    let mut bytes = [0u8; 256];
    bytes[..8].copy_from_slice(&ordinal.to_le_bytes());
    let mut handles = [HandleRef { raw: 0 }; 4];
    let encoded = value
        .encode(&mut bytes[8..], &mut handles)
        .map_err(|_| Error::Envelope)?;
    let raw_handles = handles[..encoded.handles]
        .iter()
        .map(|handle| handle.raw)
        .collect::<Vec<_>>();
    bexos_userspace::Channel(channel)
        .send(&bytes[..8 + encoded.bytes], &raw_handles)
        .map_err(|_| Error::Channel)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_notification_is_exactly_once_without_an_endpoint() {
        EVENTS.store(0, Ordering::Release);
        STOP_SENT.store(false, Ordering::Release);
        assert_eq!(stop(Status::ErrInvalidArgs, 1), Ok(()));
        assert_eq!(stop(Status::Ok, 0), Ok(()));
        assert!(STOP_SENT.load(Ordering::Acquire));
    }

    #[test]
    fn stop_state_is_monotonic() {
        STOP_REQUESTED.store(false, Ordering::Release);
        assert!(!stop_requested());
        STOP_REQUESTED.store(true, Ordering::Release);
        assert!(stop_requested());
    }

    #[test]
    fn callback_order_accepts_failed_start_and_normal_lifecycle() {
        let mut failed = EventState::default();
        assert_eq!(failed.on_stop(), Ok(()));
        assert_eq!(failed, EventState::new(false, true));

        let mut running = EventState::default();
        assert_eq!(running.on_ready(Status::Ok), Ok(()));
        assert_eq!(running.on_stop(), Ok(()));
        assert_eq!(running, EventState::new(true, true));
    }

    #[test]
    fn callback_order_rejects_duplicates_and_ready_after_stop() {
        let mut state = EventState::default();
        assert_eq!(state.on_ready(Status::ErrInvalidArgs), Err(Error::Protocol));
        assert_eq!(state.on_ready(Status::Ok), Ok(()));
        assert_eq!(state.on_ready(Status::Ok), Err(Error::Protocol));
        assert_eq!(state.on_stop(), Ok(()));
        assert_eq!(state.on_stop(), Err(Error::Protocol));

        let mut failed = EventState::default();
        assert_eq!(failed.on_stop(), Ok(()));
        assert_eq!(failed.on_ready(Status::Ok), Err(Error::Protocol));
    }

    #[test]
    fn generated_program_metadata_enforces_declared_fidl_bounds() {
        use component_runner_fidl::{FidlWireError, ProgramMetadata};

        let long_type = "x".repeat(257);
        let mut bytes = vec![0u8; 70_000];
        assert_eq!(
            ProgramMetadata {
                type_url: &long_type,
                payload: b"ok",
            }
            .encode(&mut bytes, &mut []),
            Err(FidlWireError::LimitExceeded)
        );

        let large_payload = vec![0u8; 65_537];
        assert_eq!(
            ProgramMetadata {
                type_url: "type.googleapis.com/example.Options",
                payload: &large_payload,
            }
            .encode(&mut bytes, &mut []),
            Err(FidlWireError::LimitExceeded)
        );

        let mut encoded = vec![0u8; 32 + 257];
        encoded[0..8].copy_from_slice(&32u64.to_le_bytes());
        encoded[8..16].copy_from_slice(&257u64.to_le_bytes());
        encoded[16..24].copy_from_slice(&(32u64 + 257).to_le_bytes());
        encoded[24..32].copy_from_slice(&0u64.to_le_bytes());
        encoded[32..].fill(b'x');
        assert_eq!(
            ProgramMetadata::decode(&encoded, &[]),
            Err(FidlWireError::LimitExceeded)
        );
    }

    #[test]
    fn generated_string_vector_elements_enforce_declared_fidl_bounds() {
        use component_runner_fidl::{FidlWireError, ServiceConnection};

        let mut encoded = vec![0u8; 112 + 16 + 129];
        encoded[64..72].copy_from_slice(&112u64.to_le_bytes());
        encoded[72..80].copy_from_slice(&1u64.to_le_bytes());
        encoded[105..109].copy_from_slice(&0u32.to_le_bytes());
        encoded[112..120].copy_from_slice(&16u64.to_le_bytes());
        encoded[120..128].copy_from_slice(&129u64.to_le_bytes());
        encoded[128..].fill(b'x');
        assert_eq!(
            ServiceConnection::decode(&encoded, &[HandleRef { raw: 7 }]),
            Err(FidlWireError::LimitExceeded)
        );
    }

    #[test]
    fn late_service_connection_round_trips_typed_metadata_and_endpoint() {
        use component_runner_fidl::{ServiceConnection, WireStringVector};

        let mut ordinals = Vec::new();
        ordinals.extend_from_slice(&7u64.to_le_bytes());
        ordinals.extend_from_slice(&99u64.to_le_bytes());
        let permissions = ["read", "write"];
        let request = ComponentControllerConnectRequest {
            connection: ServiceConnection {
                service: "bexos.examples.Clock",
                protocol: "Clock",
                capability: "clock.client",
                method_ordinals: &ordinals,
                permission_values: WireStringVector::from_slice(&permissions),
                caller_package: "com.example.client",
                caller_uid: 42,
                caller_foreground: true,
                endpoint: HandleRef { raw: 81 },
            },
        };
        let mut bytes = vec![0u8; 1024];
        bytes[..8].copy_from_slice(&4u64.to_le_bytes());
        let mut handles = [HandleRef { raw: 0 }; 1];
        let encoded = request
            .encode(&mut bytes[8..], &mut handles)
            .expect("encode late service connection");
        bytes.truncate(8 + encoded.bytes);

        assert_eq!(
            decode_controller_action(&bytes, &handles[..encoded.handles]),
            Ok(ControllerAction::Connect(LateServiceConnection {
                service: "bexos.examples.Clock".into(),
                protocol: "Clock".into(),
                capability: "clock.client".into(),
                method_ordinals: vec![7, 99],
                permission_values: vec!["read".into(), "write".into()],
                caller_package: "com.example.client".into(),
                caller_uid: 42,
                caller_foreground: true,
                endpoint: 81,
            }))
        );
    }

    #[test]
    fn late_service_connection_rejects_unaligned_ordinals() {
        use component_runner_fidl::{ServiceConnection, WireStringVector};

        let permissions: [&str; 0] = [];
        let request = ComponentControllerConnectRequest {
            connection: ServiceConnection {
                service: "svc",
                protocol: "protocol",
                capability: "capability",
                method_ordinals: &[1, 2, 3],
                permission_values: WireStringVector::from_slice(&permissions),
                caller_package: "client",
                caller_uid: 1,
                caller_foreground: false,
                endpoint: HandleRef { raw: 91 },
            },
        };
        let mut bytes = vec![0u8; 512];
        bytes[..8].copy_from_slice(&4u64.to_le_bytes());
        let mut handles = [HandleRef { raw: 0 }; 1];
        let encoded = request
            .encode(&mut bytes[8..], &mut handles)
            .expect("encode invalid ordinal bytes");
        bytes.truncate(8 + encoded.bytes);
        assert_eq!(
            decode_controller_action(&bytes, &handles[..encoded.handles]),
            Err(Error::Envelope)
        );
    }
}
