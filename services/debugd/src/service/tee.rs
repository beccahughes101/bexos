use super::*;

pub trait TeeManager {
    async fn concurrent_storage_probe(&mut self) -> DebugStatusResponse {
        DebugStatusResponse {
            status: -95,
            message: "queued secure probe unavailable".into(),
        }
    }
    async fn info(&mut self) -> TeeInfoResponse;
    async fn list_apps(&mut self) -> TeeAppListResponse;
    async fn install_app(&mut self, request: TeeInstallAppRequest) -> TeeInstallAppResponse;
    async fn uninstall_app(&mut self, request: TeeUuidRequest) -> DebugStatusResponse;
    async fn open_session(&mut self, request: TeeUuidRequest) -> TeeOpenSessionResponse;
    async fn close_session(&mut self, request: TeeSessionRequest) -> DebugStatusResponse;
    async fn invoke(&mut self, request: TeeInvokeRequest) -> TeeInvokeResponse;
    async fn update_core(&mut self, request: TeeUpdateCoreRequest) -> TeeUpdateCoreResponse;
    async fn update_status(&mut self) -> TeeUpdateStatusResponse;
}

pub struct UnsupportedTeeManager;

impl TeeManager for UnsupportedTeeManager {
    async fn info(&mut self) -> TeeInfoResponse {
        TeeInfoResponse {
            status: -95,
            present: false,
            kind: "unavailable".into(),
            secure_os_version: 0,
            anti_rollback_version: 0,
        }
    }

    async fn list_apps(&mut self) -> TeeAppListResponse {
        TeeAppListResponse {
            status: -95,
            apps: Vec::new(),
        }
    }

    async fn install_app(&mut self, _request: TeeInstallAppRequest) -> TeeInstallAppResponse {
        TeeInstallAppResponse {
            status: -95,
            uuid: Vec::new(),
        }
    }

    async fn uninstall_app(&mut self, _request: TeeUuidRequest) -> DebugStatusResponse {
        unsupported_tee_response()
    }

    async fn open_session(&mut self, _request: TeeUuidRequest) -> TeeOpenSessionResponse {
        TeeOpenSessionResponse {
            status: -95,
            session_id: 0,
        }
    }

    async fn close_session(&mut self, _request: TeeSessionRequest) -> DebugStatusResponse {
        unsupported_tee_response()
    }

    async fn invoke(&mut self, _request: TeeInvokeRequest) -> TeeInvokeResponse {
        TeeInvokeResponse {
            status: -95,
            response: Vec::new(),
        }
    }

    async fn update_core(&mut self, _request: TeeUpdateCoreRequest) -> TeeUpdateCoreResponse {
        TeeUpdateCoreResponse {
            status: -95,
            new_version: 0,
            message: "debugd tee backend unavailable".into(),
        }
    }

    async fn update_status(&mut self) -> TeeUpdateStatusResponse {
        TeeUpdateStatusResponse {
            status: -95,
            update_status: "Unavailable".into(),
            generation: 0,
            message: "debugd tee backend unavailable".into(),
            ..Default::default()
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BufferedTeeManager {
    pub(super) apps: Vec<TeeAppInfo>,
    pub(super) sessions: Vec<(u64, Vec<u8>)>,
    pub(super) next_session_id: u64,
    pub(super) update: TeeUpdateStatusResponse,
}

impl BufferedTeeManager {
    pub fn new() -> Self {
        Self {
            next_session_id: 1,
            update: TeeUpdateStatusResponse {
                status: 0,
                update_status: "Idle".into(),
                generation: 0,
                message: "tee update idle".into(),
                ..Default::default()
            },
            ..Self::default()
        }
    }
}

impl TeeManager for BufferedTeeManager {
    async fn info(&mut self) -> TeeInfoResponse {
        TeeInfoResponse {
            status: 0,
            present: true,
            kind: "SoftwareEmu".into(),
            secure_os_version: 1,
            anti_rollback_version: self.update.generation as u32,
        }
    }

    async fn list_apps(&mut self) -> TeeAppListResponse {
        TeeAppListResponse {
            status: 0,
            apps: self.apps.clone(),
        }
    }

    async fn install_app(&mut self, request: TeeInstallAppRequest) -> TeeInstallAppResponse {
        if request.payload.is_empty() {
            return TeeInstallAppResponse {
                status: -8,
                uuid: Vec::new(),
            };
        }
        let uuid = derive_debug_uuid(&request.payload).to_vec();
        if !self.apps.iter().any(|app| app.uuid == uuid) {
            self.apps.push(TeeAppInfo {
                uuid: uuid.clone(),
                version: request.payload.len() as u32,
                active_sessions: 0,
                entry_point_name: "debug_software_emu_ta".into(),
                storage_bytes_used: request.payload.len() as u64,
                package_id: String::new(),
                protected: false,
                package_managed: false,
            });
        }
        TeeInstallAppResponse { status: 0, uuid }
    }

    async fn uninstall_app(&mut self, request: TeeUuidRequest) -> DebugStatusResponse {
        if self.sessions.iter().any(|(_, uuid)| *uuid == request.uuid) {
            return debug_status(-2, "tee app has active sessions");
        }
        let before = self.apps.len();
        self.apps.retain(|app| app.uuid != request.uuid);
        if self.apps.len() == before {
            debug_status(-10, "tee app not found")
        } else {
            ok_response("tee app uninstalled")
        }
    }

    async fn open_session(&mut self, request: TeeUuidRequest) -> TeeOpenSessionResponse {
        let Some(app) = self.apps.iter_mut().find(|app| app.uuid == request.uuid) else {
            return TeeOpenSessionResponse {
                status: -10,
                session_id: 0,
            };
        };
        let session_id = self.next_session_id;
        self.next_session_id = self.next_session_id.saturating_add(1);
        app.active_sessions = app.active_sessions.saturating_add(1);
        self.sessions.push((session_id, request.uuid));
        TeeOpenSessionResponse {
            status: 0,
            session_id,
        }
    }

    async fn close_session(&mut self, request: TeeSessionRequest) -> DebugStatusResponse {
        let Some(index) = self
            .sessions
            .iter()
            .position(|(session_id, _)| *session_id == request.session_id)
        else {
            return debug_status(-1, "tee session not found");
        };
        let (_, uuid) = self.sessions.swap_remove(index);
        if let Some(app) = self.apps.iter_mut().find(|app| app.uuid == uuid) {
            app.active_sessions = app.active_sessions.saturating_sub(1);
        }
        ok_response("tee session closed")
    }

    async fn invoke(&mut self, request: TeeInvokeRequest) -> TeeInvokeResponse {
        if !self
            .sessions
            .iter()
            .any(|(session_id, _)| *session_id == request.session_id)
        {
            return TeeInvokeResponse {
                status: -1,
                response: Vec::new(),
            };
        }
        let mut response = Vec::new();
        response.extend_from_slice(b"BEXTEE");
        response.extend_from_slice(&request.session_id.to_le_bytes());
        response.extend_from_slice(&request.command_id.to_le_bytes());
        response.extend_from_slice(&request.payload);
        TeeInvokeResponse {
            status: 0,
            response,
        }
    }

    async fn update_core(&mut self, request: TeeUpdateCoreRequest) -> TeeUpdateCoreResponse {
        if request.generation == 0 || request.artifact_hash.len() != 32 || request.image.is_empty()
        {
            return TeeUpdateCoreResponse {
                status: -8,
                new_version: 0,
                message: "invalid tee core update".into(),
            };
        }
        self.update = TeeUpdateStatusResponse {
            status: 0,
            update_status: "Completed".into(),
            generation: request.generation,
            message: alloc::format!("tee core updated target={}", request.target),
            phase: "Completed".into(),
            ..Default::default()
        };
        TeeUpdateCoreResponse {
            status: 0,
            new_version: request.generation as u32,
            message: "tee core update completed".into(),
        }
    }

    async fn update_status(&mut self) -> TeeUpdateStatusResponse {
        self.update.clone()
    }
}
