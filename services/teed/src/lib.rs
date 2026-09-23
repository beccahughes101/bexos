#![allow(async_fn_in_trait)]

extern crate alloc;
mod storage_proxy;
use bexos_tee_driver_client::{OP_STORAGE_PROXY_HANDLER, StorageProxyHandler};

pub mod bundle;
pub mod migration;
pub mod runtime;
pub mod secure_state;

use alloc::string::String;
use alloc::vec::Vec;
use bexos_orchestrator::BootRollbackState;
use bexos_tee_driver_client::{
    Driver, OP_BUILTIN_DISCOVERY, OP_CLOSE, OP_CONNECT, OP_CORE_ACTIVATE, OP_CORE_STATUS,
    OP_INVOKE, OP_LOAD_APP, OP_PROBE, OP_RECV, OP_SEND, OP_TRANSPORT_RESET, OP_UNLOAD_APP,
    STATUS_ACCESS_DENIED, STATUS_ALREADY_EXISTS, STATUS_BUFFER_TOO_SMALL, STATUS_INVALID_ARGS,
    STATUS_NO_MEMORY, STATUS_NOT_FOUND, STATUS_OK, STATUS_PEER_CLOSED, STATUS_RESOURCE_EXHAUSTED,
    STATUS_TIMED_OUT, STATUS_UNAVAILABLE, STATUS_VERIFY_FAILED, TEE_KIND_SOFTWARE, TEE_KIND_TRUSTY,
    TeeAppDescriptor, TeeBuffer, TeeCoreImage, TeeDriverCompletion, TeeDriverError, TeeDriverInfo,
    TeeDriverRequest, TeeEndpoint, TeeOutBuffer, TeeUpdateState,
};
use bexos_trusty_client::protocol::{
    AUTHMGR_BE_PORT, AUTHMGR_BE_UUID, AVB_PORT, AVB_UUID, GATEKEEPER_PORT, GATEKEEPER_UUID,
    KEYMINT_PORT, KEYMINT_SECURE_PORT, KEYMINT_UUID, ORCHESTRATOR_PORT, ORCHESTRATOR_UUID,
    STORAGE_PROXY_PORT, STORAGE_UUID,
};
use tee_manager_fidl::{
    TaState, TeeActivationMode, TeeKind, TeeStatus, TeeUpdatePhase, TeeUpdateStatus,
};

pub use runtime::main;

pub const MAX_TA_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_TEE_IMAGE_BYTES: usize = 64 * 1024 * 1024;
const INVOKE_RESPONSE_LIMIT: usize = 64 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TeeInfo {
    pub present: bool,
    pub kind: TeeKind,
    pub secure_os_version: u32,
    pub anti_rollback_version: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrustedApp {
    pub uuid: [u8; 16],
    pub version: u32,
    pub active_sessions: u32,
    pub entry_point_name: String,
    pub storage_bytes_used: u64,
    pub package_id: String,
    pub service_ports: Vec<String>,
    pub protected: bool,
    pub package_managed: bool,
}

impl TrustedApp {
    fn fidl(&self) -> TaState<'_> {
        TaState {
            uuid: self.uuid,
            version: self.version,
            active_sessions: self.active_sessions,
            entry_point_name: &self.entry_point_name,
            storage_bytes_used: self.storage_bytes_used,
            package_id: &self.package_id,
            protected: self.protected,
            package_managed: self.package_managed,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TeeUpdateProgress {
    pub status: TeeUpdateStatus,
    pub phase: TeeUpdatePhase,
    pub active_slot: String,
    pub pending_slot: String,
    pub generation: u64,
    pub rollback_available: bool,
    pub reboot_required: bool,
    pub message: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandResult {
    pub bytes: Vec<u8>,
}

pub trait TeeBackend {
    fn rpmb_channel(&self) -> Option<bexos_userspace::Channel> {
        None
    }
    fn set_rpmb_channel(&mut self, _channel: bexos_userspace::Channel) {}
    fn attach_rpmb(&mut self, _channel: bexos_userspace::Channel) -> Result<(), TeeStatus> {
        Err(TeeStatus::ErrInvalidArgs)
    }
    fn activate_backend(&mut self) -> Result<(), TeeStatus> {
        Ok(())
    }
    fn pause_backend(&mut self) -> Result<(), TeeStatus> {
        Ok(())
    }
    fn progress_storage(&mut self) -> Result<(), TeeStatus> {
        Ok(())
    }

    async fn info(&mut self) -> Result<TeeInfo, TeeStatus>;
    async fn list_trusted_apps(&mut self) -> Result<Vec<TrustedApp>, TeeStatus>;
    async fn install_trusted_app(&mut self, payload: &[u8]) -> Result<[u8; 16], TeeStatus>;
    async fn uninstall_trusted_app(&mut self, uuid: [u8; 16]) -> TeeStatus;
    async fn activate_package_app(
        &mut self,
        package_id: &str,
        uuid: [u8; 16],
        secure_version: u64,
        ports: &[String],
        protected: bool,
        payload: &[u8],
    ) -> TeeStatus;
    async fn deactivate_package_app(&mut self, package_id: &str) -> TeeStatus;
    async fn query_package_app(&mut self, package_id: &str) -> Result<TrustedApp, TeeStatus>;
    async fn open_endpoint(
        &mut self,
        package_id: &str,
        service_port: &str,
    ) -> Result<u64, TeeStatus>;
    async fn open_session(&mut self, uuid: [u8; 16]) -> Result<u64, TeeStatus>;
    async fn close_session(&mut self, session_id: u64) -> TeeStatus;
    async fn invoke_command(
        &mut self,
        session_id: u64,
        command_id: u32,
        payload: &[u8],
    ) -> Result<CommandResult, TeeStatus>;
    async fn update_tee_core(
        &mut self,
        generation: u64,
        target: &str,
        activation: TeeActivationMode,
        artifact_hash: &[u8],
        image: &[u8],
        image_physical: u64,
    ) -> Result<u32, TeeStatus>;
    async fn update_status(&mut self) -> Result<TeeUpdateProgress, TeeStatus>;
}

pub struct DriverBackend {
    driver: Option<Driver>,
    proxy: Option<alloc::boxed::Box<storage_proxy::StorageProxy>>,
    apps: Vec<TrustedApp>,
    sessions: Vec<Session>,
    next_session_id: u64,
    next_request_id: u64,
    info: TeeInfo,
    update: TeeUpdateProgress,
}

impl core::fmt::Debug for DriverBackend {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("DriverBackend")
            .field("driver_loaded", &self.driver.is_some())
            .field("apps", &self.apps)
            .field("sessions", &self.sessions)
            .field("next_session_id", &self.next_session_id)
            .field("next_request_id", &self.next_request_id)
            .field("info", &self.info)
            .field("update", &self.update)
            .finish()
    }
}

impl DriverBackend {
    pub fn load(linker_data: &[u8]) -> Result<Self, TeeStatus> {
        let mut backend = Self::empty();
        backend.bind_driver(linker_data)?;
        Ok(backend)
    }

    fn install_proxy_handler(&mut self) -> Result<(), TeeStatus> {
        let Some(proxy) = self.proxy.as_mut() else {
            return Ok(());
        };
        let handler = proxy.handler();
        let request_id = self.alloc_request_id();
        self.request(TeeDriverRequest {
            op: OP_STORAGE_PROXY_HANDLER,
            request_id,
            input: TeeBuffer {
                ptr: (&handler as *const StorageProxyHandler).cast(),
                len: core::mem::size_of::<StorageProxyHandler>(),
                ..Default::default()
            },
            ..Default::default()
        })?;
        Ok(())
    }

    pub(crate) fn reload_driver(&mut self, linker_data: &[u8]) -> TeeStatus {
        match self.bind_driver(linker_data) {
            Ok(()) => TeeStatus::Ok,
            Err(status) => status,
        }
    }

    fn bind_driver(&mut self, linker_data: &[u8]) -> Result<(), TeeStatus> {
        let driver = Driver::load(linker_data).map_err(driver_load_status)?;
        self.driver = Some(driver);
        let request_id = self.alloc_request_id();
        let completion = self.request(TeeDriverRequest {
            op: OP_PROBE,
            request_id,
            ..Default::default()
        })?;
        self.info = tee_info(completion.info)?;
        if self.info.kind == TeeKind::ArmTrusty {
            self.apps = trusty_builtin_apps();
        }
        Ok(())
    }

    pub fn empty() -> Self {
        Self {
            driver: None,
            proxy: None,
            apps: Vec::new(),
            sessions: Vec::new(),
            next_session_id: 1,
            next_request_id: 1,
            info: TeeInfo {
                present: false,
                kind: TeeKind::SoftwareEmu,
                secure_os_version: 0,
                anti_rollback_version: 0,
            },
            update: TeeUpdateProgress {
                status: TeeUpdateStatus::Idle,
                phase: TeeUpdatePhase::Idle,
                active_slot: "A".into(),
                pending_slot: String::new(),
                generation: 0,
                rollback_available: false,
                reboot_required: false,
                message: "tee update idle".into(),
            },
        }
    }

    fn alloc_request_id(&mut self) -> u64 {
        let id = self.next_request_id;
        self.next_request_id = self.next_request_id.saturating_add(1).max(1);
        id
    }

    fn request(&mut self, request: TeeDriverRequest) -> Result<TeeDriverCompletion, TeeStatus> {
        let Some(driver) = self.driver.as_mut() else {
            return Err(TeeStatus::ErrPeerClosed);
        };
        driver.submit(&request).map_err(driver_status)?;
        loop {
            match driver.poll().map_err(driver_status)? {
                Some(completion) if completion.request_id == request.request_id => {
                    if completion.status == STATUS_OK {
                        return Ok(completion);
                    }
                    return Err(status_code(completion.status));
                }
                Some(_) => continue,
                None => return Err(TeeStatus::ErrTimedOut),
            }
        }
    }

    pub(crate) fn connect_storage_proxy(&mut self) -> Result<u64, TeeStatus> {
        let id = self.next_session_id;
        self.next_session_id = self.next_session_id.saturating_add(1).max(1);
        let request_id = self.alloc_request_id();
        self.request(TeeDriverRequest {
            op: OP_CONNECT,
            request_id,
            session_id: id,
            endpoint: TeeEndpoint {
                uuid: STORAGE_UUID,
                port_ptr: STORAGE_PROXY_PORT.as_ptr(),
                port_len: STORAGE_PROXY_PORT.len(),
                ..Default::default()
            },
            ..Default::default()
        })?;
        self.sessions.push(Session {
            id,
            uuid: STORAGE_UUID,
            port: Some(STORAGE_PROXY_PORT.into()),
        });
        Ok(id)
    }

    pub(crate) fn try_recv_raw(
        &mut self,
        session_id: u64,
        out: &mut [u8],
    ) -> Result<usize, TeeStatus> {
        if !self.sessions.iter().any(|session| session.id == session_id) {
            return Err(TeeStatus::ErrInvalidHandle);
        }
        let request_id = self.alloc_request_id();
        let completion = self.request(TeeDriverRequest {
            op: OP_RECV,
            request_id,
            session_id,
            output: TeeOutBuffer {
                ptr: out.as_mut_ptr(),
                len: out.len(),
                written: 0,
            },
            ..Default::default()
        })?;
        usize::try_from(completion.value).map_err(|_| TeeStatus::ErrBufferTooSmall)
    }

    pub(crate) fn send_raw(&mut self, session_id: u64, bytes: &[u8]) -> TeeStatus {
        if !self.sessions.iter().any(|session| session.id == session_id) {
            return TeeStatus::ErrInvalidHandle;
        }
        let request_id = self.alloc_request_id();
        self.request(TeeDriverRequest {
            op: OP_SEND,
            request_id,
            session_id,
            input: TeeBuffer {
                ptr: bytes.as_ptr(),
                len: bytes.len(),
                ..Default::default()
            },
            ..Default::default()
        })
        .map(|_| TeeStatus::Ok)
        .unwrap_or_else(|status| status)
    }

    pub(crate) fn reconnect_catalog(&mut self) -> TeeStatus {
        let request_id = self.alloc_request_id();
        match self.request(TeeDriverRequest {
            op: OP_TRANSPORT_RESET,
            request_id,
            ..Default::default()
        }) {
            Ok(_) => {}
            Err(status) => return status,
        }
        let apps = self.apps.clone();
        for app in apps {
            // Built-in Trusty apps remain installed across a normal-world
            // handover. Only the software backend supports dynamic loading.
            if self.info.kind == TeeKind::ArmTrusty {
                continue;
            }
            let request_id = self.alloc_request_id();
            if let Err(status) = self.request(TeeDriverRequest {
                op: OP_LOAD_APP,
                request_id,
                app: TeeAppDescriptor {
                    uuid: app.uuid,
                    version: app.version as u64,
                    ..Default::default()
                },
                ..Default::default()
            }) {
                return status;
            }
        }
        let sessions = self.sessions.clone();
        self.sessions.clear();
        for session in sessions {
            let port = session.port.as_deref().unwrap_or_default();
            let request_id = self.alloc_request_id();
            match self.request(TeeDriverRequest {
                op: OP_CONNECT,
                request_id,
                session_id: session.id,
                endpoint: TeeEndpoint {
                    uuid: session.uuid,
                    port_ptr: port.as_ptr(),
                    port_len: port.len(),
                    ..Default::default()
                },
                ..Default::default()
            }) {
                Ok(_) => self.sessions.push(session),
                Err(status) => return status,
            }
        }
        TeeStatus::Ok
    }

    pub fn snapshot_state(&self) -> SoftwareEmuBackend {
        SoftwareEmuBackend::from_parts(
            self.apps.clone(),
            self.sessions
                .iter()
                .map(|session| (session.id, session.uuid, session.port.clone()))
                .collect(),
            self.next_session_id,
            self.info.secure_os_version,
            self.info.anti_rollback_version,
            self.update.clone(),
        )
    }

    pub fn restore_state(&mut self, state: SoftwareEmuBackend) {
        let (apps, sessions, next_session_id, secure_os_version, anti_rollback_version, update) =
            state.parts();
        self.apps = apps.to_vec();
        self.sessions = sessions
            .into_iter()
            .map(|(id, uuid, port)| Session { id, uuid, port })
            .collect();
        self.next_session_id = next_session_id;
        self.info.secure_os_version = secure_os_version;
        self.info.anti_rollback_version = anti_rollback_version;
        self.update = update.clone();
    }
}

impl Default for DriverBackend {
    fn default() -> Self {
        Self::empty()
    }
}

impl TeeBackend for DriverBackend {
    fn progress_storage(&mut self) -> Result<(), TeeStatus> {
        if self.proxy.is_none() {
            return Ok(());
        }
        let Some(session) = self.sessions.iter().find(|s| s.uuid == STORAGE_UUID) else {
            return Ok(());
        };
        let id = session.id;
        let mut request = [0; 8192];
        let count = match self.try_recv_raw(id, &mut request) {
            Ok(count) => count,
            Err(TeeStatus::ErrTimedOut) => return Ok(()),
            Err(status) => return Err(status),
        };
        let channel = self.proxy.as_ref().unwrap().channel;
        let response = bexos_rpmb_proxy::dispatch(&request[..count], |frames, count| {
            bexos_rpmb_proxy::exchange(channel, frames, count)
        })
        .map_err(|_| TeeStatus::ErrPeerClosed)?;
        match self.send_raw(id, &response) {
            TeeStatus::Ok => Ok(()),
            status => Err(status),
        }
    }
    fn pause_backend(&mut self) -> Result<(), TeeStatus> {
        // Release the old QL-TIPC device before another process reconnects the
        // single-owner storage proxy. Logical sessions remain in the snapshot.
        if self.driver.is_none() {
            return Ok(());
        }
        // Reset explicitly first so shutdown errors reach the migration loop
        // instead of being discarded by the driver's Drop implementation.
        let request_id = self.alloc_request_id();
        let result = self.request(TeeDriverRequest {
            op: OP_TRANSPORT_RESET,
            request_id,
            ..Default::default()
        });
        self.driver = None;
        result.map(|_| ())
    }

    fn rpmb_channel(&self) -> Option<bexos_userspace::Channel> {
        self.proxy.as_ref().map(|p| p.channel)
    }
    fn set_rpmb_channel(&mut self, channel: bexos_userspace::Channel) {
        self.proxy = Some(alloc::boxed::Box::new(storage_proxy::StorageProxy {
            channel,
        }));
    }
    fn attach_rpmb(&mut self, channel: bexos_userspace::Channel) -> Result<(), TeeStatus> {
        self.set_rpmb_channel(channel);
        self.install_proxy_handler()?;
        self.connect_storage_proxy()?;
        Ok(())
    }
    fn activate_backend(&mut self) -> Result<(), TeeStatus> {
        self.bind_driver(&[])?;
        self.install_proxy_handler()?;
        // Restore the proxy before clients whose connections need storage.
        self.sessions
            .sort_by_key(|session| session.port.as_deref() != Some(STORAGE_PROXY_PORT));
        let status = self.reconnect_catalog();
        if status == TeeStatus::Ok {
            Ok(())
        } else {
            Err(status)
        }
    }

    async fn info(&mut self) -> Result<TeeInfo, TeeStatus> {
        if !self.info.present {
            let request_id = self.alloc_request_id();
            let completion = self.request(TeeDriverRequest {
                op: OP_PROBE,
                request_id,
                ..Default::default()
            })?;
            self.info = tee_info(completion.info)?;
        }
        Ok(self.info.clone())
    }

    async fn list_trusted_apps(&mut self) -> Result<Vec<TrustedApp>, TeeStatus> {
        let request_id = self.alloc_request_id();
        let _ = self.request(TeeDriverRequest {
            op: OP_BUILTIN_DISCOVERY,
            request_id,
            ..Default::default()
        })?;
        Ok(self.apps.clone())
    }

    async fn install_trusted_app(&mut self, payload: &[u8]) -> Result<[u8; 16], TeeStatus> {
        let uuid = derive_uuid(payload);
        let version = payload.len() as u32;
        let request_id = self.alloc_request_id();
        self.request(TeeDriverRequest {
            op: OP_LOAD_APP,
            request_id,
            app: TeeAppDescriptor {
                uuid,
                version: u64::from(version),
                ..Default::default()
            },
            input: TeeBuffer {
                ptr: payload.as_ptr(),
                len: payload.len(),
                ..Default::default()
            },
            ..Default::default()
        })?;
        self.apps.push(TrustedApp {
            uuid,
            version,
            active_sessions: 0,
            entry_point_name: "package-managed-trusty-app".into(),
            storage_bytes_used: payload.len() as u64,
            package_id: String::new(),
            service_ports: Vec::new(),
            protected: false,
            package_managed: false,
        });
        Ok(uuid)
    }

    async fn uninstall_trusted_app(&mut self, uuid: [u8; 16]) -> TeeStatus {
        if self.sessions.iter().any(|session| session.uuid == uuid) {
            return TeeStatus::ErrAccessDenied;
        }
        let request_id = self.alloc_request_id();
        match self.request(TeeDriverRequest {
            op: OP_UNLOAD_APP,
            request_id,
            app: TeeAppDescriptor {
                uuid,
                ..Default::default()
            },
            ..Default::default()
        }) {
            Ok(_) => {
                self.apps.retain(|app| app.uuid != uuid);
                TeeStatus::Ok
            }
            Err(status) => status,
        }
    }

    async fn activate_package_app(
        &mut self,
        package_id: &str,
        uuid: [u8; 16],
        secure_version: u64,
        ports: &[String],
        protected: bool,
        payload: &[u8],
    ) -> TeeStatus {
        if self.apps.iter().any(|app| app.package_id == package_id)
            || self.apps.iter().any(|app| app.uuid == uuid)
        {
            return TeeStatus::ErrAlreadyExists;
        }
        let request_id = self.alloc_request_id();
        match self.request(TeeDriverRequest {
            op: OP_LOAD_APP,
            request_id,
            app: TeeAppDescriptor {
                uuid,
                version: secure_version,
                package_id_ptr: package_id.as_ptr(),
                package_id_len: package_id.len(),
                protected_policy: u32::from(protected),
                ..Default::default()
            },
            input: TeeBuffer {
                ptr: payload.as_ptr(),
                len: payload.len(),
                ..Default::default()
            },
            ..Default::default()
        }) {
            Ok(_) => {
                self.apps.push(TrustedApp {
                    uuid,
                    version: secure_version.min(u64::from(u32::MAX)) as u32,
                    active_sessions: 0,
                    entry_point_name: ports.first().cloned().unwrap_or_default(),
                    storage_bytes_used: payload.len() as u64,
                    package_id: package_id.into(),
                    service_ports: ports.to_vec(),
                    protected,
                    package_managed: true,
                });
                TeeStatus::Ok
            }
            Err(status) => status,
        }
    }

    async fn deactivate_package_app(&mut self, package_id: &str) -> TeeStatus {
        let Some(app) = self
            .apps
            .iter()
            .find(|app| app.package_id == package_id)
            .cloned()
        else {
            return TeeStatus::ErrNotFound;
        };
        if app.active_sessions != 0 || self.sessions.iter().any(|session| session.uuid == app.uuid)
        {
            return TeeStatus::ErrAccessDenied;
        }
        self.uninstall_trusted_app(app.uuid).await
    }

    async fn query_package_app(&mut self, package_id: &str) -> Result<TrustedApp, TeeStatus> {
        self.apps
            .iter()
            .find(|app| app.package_id == package_id)
            .cloned()
            .ok_or(TeeStatus::ErrNotFound)
    }

    async fn open_endpoint(
        &mut self,
        package_id: &str,
        service_port: &str,
    ) -> Result<u64, TeeStatus> {
        let app_uuid = self
            .apps
            .iter()
            .find(|app| app.package_id == package_id)
            .ok_or(TeeStatus::ErrNotFound)
            .and_then(|app| {
                if app.service_ports.iter().any(|port| port == service_port) {
                    Ok(app.uuid)
                } else {
                    Err(TeeStatus::ErrAccessDenied)
                }
            })?;
        let id = self.next_session_id;
        self.next_session_id = self.next_session_id.saturating_add(1).max(1);
        let request_id = self.alloc_request_id();
        self.request(TeeDriverRequest {
            op: OP_CONNECT,
            request_id,
            session_id: id,
            endpoint: TeeEndpoint {
                uuid: app_uuid,
                port_ptr: service_port.as_ptr(),
                port_len: service_port.len(),
                ..Default::default()
            },
            ..Default::default()
        })?;
        let app = self
            .apps
            .iter_mut()
            .find(|app| app.uuid == app_uuid)
            .ok_or(TeeStatus::ErrNotFound)?;
        app.active_sessions = app.active_sessions.saturating_add(1);
        self.sessions.push(Session {
            id,
            uuid: app_uuid,
            port: Some(service_port.into()),
        });
        Ok(id)
    }

    async fn open_session(&mut self, uuid: [u8; 16]) -> Result<u64, TeeStatus> {
        let id = self.next_session_id;
        self.next_session_id = self.next_session_id.saturating_add(1).max(1);
        let request_id = self.alloc_request_id();
        self.request(TeeDriverRequest {
            op: OP_CONNECT,
            request_id,
            session_id: id,
            endpoint: TeeEndpoint {
                uuid,
                ..Default::default()
            },
            ..Default::default()
        })?;
        let app = self
            .apps
            .iter_mut()
            .find(|app| app.uuid == uuid)
            .ok_or(TeeStatus::ErrNotFound)?;
        app.active_sessions = app.active_sessions.saturating_add(1);
        self.sessions.push(Session {
            id,
            uuid,
            port: None,
        });
        Ok(id)
    }

    async fn close_session(&mut self, session_id: u64) -> TeeStatus {
        let Some(index) = self
            .sessions
            .iter()
            .position(|session| session.id == session_id)
        else {
            return TeeStatus::ErrInvalidHandle;
        };
        let request_id = self.alloc_request_id();
        let status = self
            .request(TeeDriverRequest {
                op: OP_CLOSE,
                request_id,
                session_id,
                ..Default::default()
            })
            .map(|_| TeeStatus::Ok)
            .unwrap_or_else(|status| status);
        if status == TeeStatus::Ok {
            let session = self.sessions.swap_remove(index);
            if let Some(app) = self.apps.iter_mut().find(|app| app.uuid == session.uuid) {
                app.active_sessions = app.active_sessions.saturating_sub(1);
            }
        }
        status
    }

    async fn invoke_command(
        &mut self,
        session_id: u64,
        command_id: u32,
        payload: &[u8],
    ) -> Result<CommandResult, TeeStatus> {
        if !self.sessions.iter().any(|session| session.id == session_id) {
            return Err(TeeStatus::ErrInvalidHandle);
        }
        let request_id = self.alloc_request_id();
        let mut out = alloc::vec![0; INVOKE_RESPONSE_LIMIT];
        let completion = self.request(TeeDriverRequest {
            op: OP_INVOKE,
            request_id,
            session_id,
            command_id,
            input: TeeBuffer {
                ptr: payload.as_ptr(),
                len: payload.len(),
                ..Default::default()
            },
            output: TeeOutBuffer {
                ptr: out.as_mut_ptr(),
                len: out.len(),
                written: 0,
            },
            ..Default::default()
        })?;
        let written = usize::try_from(completion.value).unwrap_or(payload.len().saturating_add(22));
        out.truncate(written.min(out.len()));
        Ok(CommandResult { bytes: out })
    }

    async fn update_tee_core(
        &mut self,
        generation: u64,
        target: &str,
        activation: TeeActivationMode,
        artifact_hash: &[u8],
        image: &[u8],
        image_physical: u64,
    ) -> Result<u32, TeeStatus> {
        let mut hash = [0; 32];
        hash.copy_from_slice(artifact_hash);
        let request_id = self.alloc_request_id();
        let completion = self.request(TeeDriverRequest {
            op: OP_CORE_ACTIVATE,
            request_id,
            core: TeeCoreImage {
                generation,
                activation: activation as u32,
                target_ptr: target.as_ptr(),
                target_len: target.len(),
                hash,
                image: TeeBuffer {
                    ptr: image.as_ptr(),
                    len: image.len(),
                    physical: image_physical,
                    ..Default::default()
                },
            },
            ..Default::default()
        })?;
        self.update = update_progress(
            completion.update,
            if completion.update.phase == 5 {
                "firmware authenticated and pending reboot"
            } else {
                "firmware activation committed"
            },
        );
        Ok(completion.value as u32)
    }

    async fn update_status(&mut self) -> Result<TeeUpdateProgress, TeeStatus> {
        let request_id = self.alloc_request_id();
        match self.request(TeeDriverRequest {
            op: OP_CORE_STATUS,
            request_id,
            ..Default::default()
        }) {
            Ok(completion) => {
                self.update = update_progress(completion.update, "tee update status available");
                Ok(self.update.clone())
            }
            Err(status) => Err(status),
        }
    }
}

fn driver_load_status(error: TeeDriverError) -> TeeStatus {
    match error {
        TeeDriverError::AbiMismatch | TeeDriverError::MissingSymbol | TeeDriverError::Storage => {
            TeeStatus::ErrVerifyFailed
        }
        TeeDriverError::Driver(status) => status_code(status),
    }
}

fn driver_status(error: TeeDriverError) -> TeeStatus {
    match error {
        TeeDriverError::Driver(status) => status_code(status),
        TeeDriverError::AbiMismatch | TeeDriverError::MissingSymbol | TeeDriverError::Storage => {
            TeeStatus::ErrPeerClosed
        }
    }
}

fn status_code(status: i32) -> TeeStatus {
    match status {
        STATUS_OK => TeeStatus::Ok,
        STATUS_INVALID_ARGS => TeeStatus::ErrInvalidArgs,
        STATUS_ACCESS_DENIED => TeeStatus::ErrAccessDenied,
        STATUS_NO_MEMORY => TeeStatus::ErrNoMemory,
        STATUS_BUFFER_TOO_SMALL => TeeStatus::ErrBufferTooSmall,
        STATUS_PEER_CLOSED => TeeStatus::ErrPeerClosed,
        STATUS_TIMED_OUT => TeeStatus::ErrTimedOut,
        STATUS_ALREADY_EXISTS => TeeStatus::ErrAlreadyExists,
        STATUS_RESOURCE_EXHAUSTED => TeeStatus::ErrResourceExhausted,
        STATUS_NOT_FOUND => TeeStatus::ErrNotFound,
        STATUS_VERIFY_FAILED => TeeStatus::ErrVerifyFailed,
        STATUS_UNAVAILABLE => TeeStatus::ErrUnavailable,
        _ => TeeStatus::ErrInvalidArgs,
    }
}

fn tee_info(info: TeeDriverInfo) -> Result<TeeInfo, TeeStatus> {
    let kind = match info.kind {
        TEE_KIND_SOFTWARE => TeeKind::SoftwareEmu,
        TEE_KIND_TRUSTY => TeeKind::ArmTrusty,
        _ => return Err(TeeStatus::ErrInvalidArgs),
    };
    Ok(TeeInfo {
        present: info.present != 0,
        kind,
        secure_os_version: info.secure_os_version,
        anti_rollback_version: info.anti_rollback_version,
    })
}

fn update_progress(update: TeeUpdateState, message: &str) -> TeeUpdateProgress {
    TeeUpdateProgress {
        status: match update.status {
            2 => TeeUpdateStatus::Staged,
            3 => TeeUpdateStatus::Applying,
            4 => TeeUpdateStatus::Completed,
            5 => TeeUpdateStatus::Failed,
            _ => TeeUpdateStatus::Idle,
        },
        phase: match update.phase {
            2 => TeeUpdatePhase::Verifying,
            3 => TeeUpdatePhase::Staged,
            4 => TeeUpdatePhase::LiveSwitch,
            5 => TeeUpdatePhase::RebootPending,
            6 => TeeUpdatePhase::HealthWindow,
            7 => TeeUpdatePhase::Completed,
            8 => TeeUpdatePhase::RolledBack,
            9 => TeeUpdatePhase::Failed,
            10 => TeeUpdatePhase::RecoveryRequired,
            _ => TeeUpdatePhase::Idle,
        },
        active_slot: array_text(&update.active_slot),
        pending_slot: array_text(&update.pending_slot),
        generation: update.generation,
        rollback_available: update.rollback_available != 0,
        reboot_required: update.reboot_required != 0,
        message: message.into(),
    }
}

fn trusty_builtin_apps() -> Vec<TrustedApp> {
    vec![
        builtin_app(
            KEYMINT_UUID,
            "bexos.ta.keymint",
            "com.android.trusty.keymint",
            &[KEYMINT_PORT, KEYMINT_SECURE_PORT],
        ),
        builtin_app(
            GATEKEEPER_UUID,
            "bexos.ta.gatekeeper",
            "com.android.trusty.gatekeeper",
            &[GATEKEEPER_PORT],
        ),
        builtin_app(
            AVB_UUID,
            "bexos.ta.avb",
            "com.android.trusty.avb",
            &[AVB_PORT],
        ),
        builtin_app(
            AUTHMGR_BE_UUID,
            "bexos.ta.authmgr",
            "com.android.trusty.authmgr.be",
            &[AUTHMGR_BE_PORT],
        ),
        builtin_app(
            STORAGE_UUID,
            "bexos.ta.storage",
            "com.android.trusty.storage",
            &[STORAGE_PROXY_PORT],
        ),
        builtin_app(
            ORCHESTRATOR_UUID,
            "bexos.orchestrator",
            "com.bexos.orchestrator",
            &[ORCHESTRATOR_PORT, "com.bexos.package-state"],
        ),
    ]
}

fn builtin_app(uuid: [u8; 16], package_id: &str, entry: &str, ports: &[&str]) -> TrustedApp {
    TrustedApp {
        uuid,
        version: 1,
        active_sessions: 0,
        entry_point_name: entry.into(),
        storage_bytes_used: 0,
        package_id: package_id.into(),
        service_ports: ports.iter().map(|port| (*port).into()).collect(),
        protected: true,
        package_managed: false,
    }
}

fn array_text(bytes: &[u8]) -> String {
    let len = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..len]).into_owned()
}

#[derive(Debug)]
pub struct TeeService<B> {
    backend: B,
}

impl<B: TeeBackend> TeeService<B> {
    pub fn new(backend: B) -> Self {
        Self { backend }
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    pub async fn info(&mut self) -> Result<TeeInfo, TeeStatus> {
        self.backend.info().await
    }

    pub async fn list_apps(&mut self) -> Result<Vec<TrustedApp>, TeeStatus> {
        self.backend.list_trusted_apps().await
    }

    pub async fn install_app(&mut self, payload: &[u8]) -> Result<[u8; 16], TeeStatus> {
        if payload.is_empty() || payload.len() > MAX_TA_BYTES {
            return Err(TeeStatus::ErrInvalidArgs);
        }
        self.backend.install_trusted_app(payload).await
    }

    pub async fn uninstall_app(&mut self, uuid: [u8; 16]) -> TeeStatus {
        self.backend.uninstall_trusted_app(uuid).await
    }

    pub async fn activate_package_app(
        &mut self,
        package_id: &str,
        provider: &str,
        uuid: [u8; 16],
        secure_version: u64,
        ports: &[String],
        protected: bool,
        payload: &[u8],
    ) -> TeeStatus {
        if provider != "trusty"
            || package_id.is_empty()
            || secure_version == 0
            || ports.is_empty()
            || ports.iter().any(|port| port.is_empty())
            || payload.is_empty()
            || payload.len() > MAX_TA_BYTES
        {
            return TeeStatus::ErrInvalidArgs;
        }
        self.backend
            .activate_package_app(package_id, uuid, secure_version, ports, protected, payload)
            .await
    }

    pub async fn deactivate_package_app(&mut self, package_id: &str) -> TeeStatus {
        self.backend.deactivate_package_app(package_id).await
    }

    pub async fn query_package_app(&mut self, package_id: &str) -> Result<TrustedApp, TeeStatus> {
        self.backend.query_package_app(package_id).await
    }

    pub async fn open_endpoint(
        &mut self,
        package_id: &str,
        service_port: &str,
    ) -> Result<u64, TeeStatus> {
        if service_port == "com.bexos.package-state" {
            return Err(TeeStatus::ErrAccessDenied);
        }
        if package_id.is_empty()
            || service_port.is_empty()
            || service_port.bytes().any(|byte| byte <= 32 || byte >= 127)
        {
            return Err(TeeStatus::ErrInvalidArgs);
        }
        self.backend.open_endpoint(package_id, service_port).await
    }

    pub async fn open_session(&mut self, uuid: [u8; 16]) -> Result<u64, TeeStatus> {
        self.backend.open_session(uuid).await
    }

    pub async fn close_session(&mut self, session_id: u64) -> TeeStatus {
        self.backend.close_session(session_id).await
    }

    pub async fn invoke(
        &mut self,
        session_id: u64,
        command_id: u32,
        payload: &[u8],
    ) -> Result<CommandResult, TeeStatus> {
        let result = self
            .backend
            .invoke_command(session_id, command_id, payload)
            .await?;
        if result.bytes.len() > INVOKE_RESPONSE_LIMIT {
            return Err(TeeStatus::ErrBufferTooSmall);
        }
        Ok(result)
    }

    pub async fn update_core(
        &mut self,
        generation: u64,
        target: &str,
        activation: TeeActivationMode,
        artifact_hash: &[u8],
        image: &[u8],
        image_physical: u64,
    ) -> Result<u32, TeeStatus> {
        if generation == 0
            || target.is_empty()
            || artifact_hash.len() != 32
            || image.is_empty()
            || image.len() > MAX_TEE_IMAGE_BYTES
        {
            return Err(TeeStatus::ErrInvalidArgs);
        }
        self.backend
            .update_tee_core(
                generation,
                target,
                activation,
                artifact_hash,
                image,
                image_physical,
            )
            .await
    }

    pub async fn update_status(&mut self) -> Result<TeeUpdateProgress, TeeStatus> {
        self.backend.update_status().await
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SoftwareEmuBackend {
    apps: Vec<TrustedApp>,
    sessions: Vec<Session>,
    next_session_id: u64,
    secure_os_version: u32,
    anti_rollback_version: u32,
    rollback: BootRollbackState,
    update: TeeUpdateProgress,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Session {
    id: u64,
    uuid: [u8; 16],
    port: Option<String>,
}

impl SoftwareEmuBackend {
    pub fn new() -> Self {
        Self {
            apps: Vec::new(),
            sessions: Vec::new(),
            next_session_id: 1,
            secure_os_version: 1,
            anti_rollback_version: 0,
            rollback: BootRollbackState::new(0, [0; 32], [0; 32]),
            update: TeeUpdateProgress {
                status: TeeUpdateStatus::Idle,
                phase: TeeUpdatePhase::Idle,
                active_slot: "A".into(),
                pending_slot: String::new(),
                generation: 0,
                rollback_available: false,
                reboot_required: false,
                message: "tee update idle".into(),
            },
        }
    }

    fn installed_mut(&mut self, uuid: [u8; 16]) -> Option<&mut TrustedApp> {
        self.apps.iter_mut().find(|app| app.uuid == uuid)
    }

    pub fn from_parts(
        apps: Vec<TrustedApp>,
        sessions: Vec<(u64, [u8; 16], Option<String>)>,
        next_session_id: u64,
        secure_os_version: u32,
        anti_rollback_version: u32,
        update: TeeUpdateProgress,
    ) -> Self {
        Self {
            apps,
            sessions: sessions
                .into_iter()
                .map(|(id, uuid, port)| Session { id, uuid, port })
                .collect(),
            next_session_id,
            secure_os_version,
            anti_rollback_version,
            rollback: BootRollbackState::new(u64::from(anti_rollback_version), [0; 32], [0; 32]),
            update,
        }
    }

    pub fn parts(
        &self,
    ) -> (
        &[TrustedApp],
        Vec<(u64, [u8; 16], Option<String>)>,
        u64,
        u32,
        u32,
        &TeeUpdateProgress,
    ) {
        (
            &self.apps,
            self.sessions
                .iter()
                .map(|session| (session.id, session.uuid, session.port.clone()))
                .collect(),
            self.next_session_id,
            self.secure_os_version,
            self.anti_rollback_version,
            &self.update,
        )
    }
}

impl Default for SoftwareEmuBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl TeeBackend for SoftwareEmuBackend {
    async fn info(&mut self) -> Result<TeeInfo, TeeStatus> {
        Ok(TeeInfo {
            present: true,
            kind: TeeKind::SoftwareEmu,
            secure_os_version: self.secure_os_version,
            anti_rollback_version: self.anti_rollback_version,
        })
    }

    async fn list_trusted_apps(&mut self) -> Result<Vec<TrustedApp>, TeeStatus> {
        Ok(self.apps.clone())
    }

    async fn install_trusted_app(&mut self, payload: &[u8]) -> Result<[u8; 16], TeeStatus> {
        if payload.is_empty() {
            return Err(TeeStatus::ErrInvalidArgs);
        }
        let uuid = derive_uuid(payload);
        if self.apps.iter().any(|app| app.uuid == uuid) {
            return Err(TeeStatus::ErrAlreadyExists);
        }
        self.apps.push(TrustedApp {
            uuid,
            version: payload.len() as u32,
            active_sessions: 0,
            entry_point_name: "software_emu_ta".into(),
            storage_bytes_used: payload.len() as u64,
            package_id: String::new(),
            service_ports: Vec::new(),
            protected: false,
            package_managed: false,
        });
        Ok(uuid)
    }

    async fn uninstall_trusted_app(&mut self, uuid: [u8; 16]) -> TeeStatus {
        if self.sessions.iter().any(|session| session.uuid == uuid) {
            return TeeStatus::ErrAccessDenied;
        }
        let before = self.apps.len();
        self.apps.retain(|app| app.uuid != uuid);
        if self.apps.len() == before {
            TeeStatus::ErrNotFound
        } else {
            TeeStatus::Ok
        }
    }

    async fn activate_package_app(
        &mut self,
        package_id: &str,
        uuid: [u8; 16],
        secure_version: u64,
        ports: &[String],
        protected: bool,
        payload: &[u8],
    ) -> TeeStatus {
        if payload.is_empty() {
            return TeeStatus::ErrInvalidArgs;
        }
        if self.apps.iter().any(|app| app.package_id == package_id)
            || self.apps.iter().any(|app| app.uuid == uuid)
        {
            return TeeStatus::ErrAlreadyExists;
        }
        self.apps.push(TrustedApp {
            uuid,
            version: secure_version.min(u64::from(u32::MAX)) as u32,
            active_sessions: 0,
            entry_point_name: ports.first().cloned().unwrap_or_default(),
            storage_bytes_used: payload.len() as u64,
            package_id: package_id.into(),
            service_ports: ports.to_vec(),
            protected,
            package_managed: true,
        });
        TeeStatus::Ok
    }

    async fn deactivate_package_app(&mut self, package_id: &str) -> TeeStatus {
        let Some(app) = self
            .apps
            .iter()
            .find(|app| app.package_id == package_id)
            .cloned()
        else {
            return TeeStatus::ErrNotFound;
        };
        if app.active_sessions != 0 || self.sessions.iter().any(|session| session.uuid == app.uuid)
        {
            return TeeStatus::ErrAccessDenied;
        }
        self.uninstall_trusted_app(app.uuid).await
    }

    async fn query_package_app(&mut self, package_id: &str) -> Result<TrustedApp, TeeStatus> {
        self.apps
            .iter()
            .find(|app| app.package_id == package_id)
            .cloned()
            .ok_or(TeeStatus::ErrNotFound)
    }

    async fn open_endpoint(
        &mut self,
        package_id: &str,
        service_port: &str,
    ) -> Result<u64, TeeStatus> {
        let app_uuid = self
            .apps
            .iter()
            .find(|app| app.package_id == package_id)
            .ok_or(TeeStatus::ErrNotFound)
            .and_then(|app| {
                if app.service_ports.iter().any(|port| port == service_port) {
                    Ok(app.uuid)
                } else {
                    Err(TeeStatus::ErrAccessDenied)
                }
            })?;
        let id = self.next_session_id;
        self.next_session_id = self.next_session_id.saturating_add(1);
        let app = self.installed_mut(app_uuid).ok_or(TeeStatus::ErrNotFound)?;
        app.active_sessions = app.active_sessions.saturating_add(1);
        self.sessions.push(Session {
            id,
            uuid: app_uuid,
            port: Some(service_port.into()),
        });
        Ok(id)
    }

    async fn open_session(&mut self, uuid: [u8; 16]) -> Result<u64, TeeStatus> {
        let id = self.next_session_id;
        self.next_session_id = self.next_session_id.saturating_add(1);
        let app = self.installed_mut(uuid).ok_or(TeeStatus::ErrNotFound)?;
        app.active_sessions = app.active_sessions.saturating_add(1);
        self.sessions.push(Session {
            id,
            uuid,
            port: None,
        });
        Ok(id)
    }

    async fn close_session(&mut self, session_id: u64) -> TeeStatus {
        let Some(index) = self
            .sessions
            .iter()
            .position(|session| session.id == session_id)
        else {
            return TeeStatus::ErrInvalidHandle;
        };
        let session = self.sessions.swap_remove(index);
        if let Some(app) = self.installed_mut(session.uuid) {
            app.active_sessions = app.active_sessions.saturating_sub(1);
        }
        TeeStatus::Ok
    }

    async fn invoke_command(
        &mut self,
        session_id: u64,
        command_id: u32,
        payload: &[u8],
    ) -> Result<CommandResult, TeeStatus> {
        if !self.sessions.iter().any(|session| session.id == session_id) {
            return Err(TeeStatus::ErrInvalidHandle);
        }
        let mut out = Vec::new();
        out.extend_from_slice(b"BEXTEE");
        out.extend_from_slice(&session_id.to_le_bytes());
        out.extend_from_slice(&command_id.to_le_bytes());
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        out.extend_from_slice(payload);
        Ok(CommandResult { bytes: out })
    }

    async fn update_tee_core(
        &mut self,
        generation: u64,
        target: &str,
        activation: TeeActivationMode,
        artifact_hash: &[u8],
        image: &[u8],
        image_physical: u64,
    ) -> Result<u32, TeeStatus> {
        let _ = image_physical;
        if target != "qemu-aarch64-tee" && target != "software-tee" {
            self.update = TeeUpdateProgress {
                status: TeeUpdateStatus::Failed,
                phase: TeeUpdatePhase::Failed,
                active_slot: self.update.active_slot.clone(),
                pending_slot: String::new(),
                generation,
                rollback_available: self.update.rollback_available,
                reboot_required: false,
                message: "tee update target rejected".into(),
            };
            return Err(TeeStatus::ErrInvalidArgs);
        }
        if generation <= u64::from(self.anti_rollback_version) {
            self.update = TeeUpdateProgress {
                status: TeeUpdateStatus::Failed,
                phase: TeeUpdatePhase::RolledBack,
                active_slot: self.update.active_slot.clone(),
                pending_slot: String::new(),
                generation,
                rollback_available: self.update.rollback_available,
                reboot_required: false,
                message: "tee generation floor rejected".into(),
            };
            return Err(TeeStatus::ErrVerifyFailed);
        }
        let next_slot = if self.update.active_slot == "A" {
            "B"
        } else {
            "A"
        };
        let mut hash = [0; 32];
        hash.copy_from_slice(artifact_hash);
        self.rollback
            .stage_update(generation, hash)
            .map_err(|_| TeeStatus::ErrAccessDenied)?;
        self.secure_os_version = self.secure_os_version.saturating_add(1);
        self.anti_rollback_version = self.anti_rollback_version.max(generation as u32);
        self.rollback
            .commit_update()
            .map_err(|_| TeeStatus::ErrInvalidArgs)?;
        self.update = TeeUpdateProgress {
            status: TeeUpdateStatus::Completed,
            phase: if activation == TeeActivationMode::OnReboot {
                TeeUpdatePhase::RebootPending
            } else {
                TeeUpdatePhase::Completed
            },
            active_slot: if activation == TeeActivationMode::OnReboot {
                self.update.active_slot.clone()
            } else {
                next_slot.into()
            },
            pending_slot: if activation == TeeActivationMode::OnReboot {
                next_slot.into()
            } else {
                String::new()
            },
            generation,
            rollback_available: true,
            reboot_required: activation == TeeActivationMode::OnReboot,
            message: alloc::format!("tee core updated mode={activation:?} bytes={}", image.len()),
        };
        Ok(self.secure_os_version)
    }

    async fn update_status(&mut self) -> Result<TeeUpdateProgress, TeeStatus> {
        Ok(self.update.clone())
    }
}

pub fn trusted_apps_to_fidl(apps: &[TrustedApp]) -> Vec<TaState<'_>> {
    apps.iter().map(TrustedApp::fidl).collect()
}

pub fn derive_uuid(payload: &[u8]) -> [u8; 16] {
    let mut uuid = *b"BEXOS-TEE-EMU-v1";
    for (index, byte) in payload.iter().enumerate() {
        uuid[index % 16] = uuid[index % 16].rotate_left(1) ^ *byte;
    }
    uuid
}
