use super::*;

pub(crate) struct TeeServiceManager {
    pub(crate) channel: Channel,
}

impl TeeServiceManager {
    pub(crate) fn new(manager: Channel) -> Self {
        let channel = match Channel::pair() {
            Ok((client, server)) => {
                if manager
                    .send(
                        b"bexos.tee.TeeManager|TeeManager|Public|1,2,3,4,5,6,7,8,9",
                        &[server.0],
                    )
                    .is_ok()
                {
                    client
                } else {
                    let _ = Memory::close(client.0);
                    let _ = Memory::close(server.0);
                    Channel(0)
                }
            }
            Err(_) => Channel(0),
        };
        Self { channel }
    }

    pub(crate) fn from_client(channel: Channel) -> Self {
        Self { channel }
    }

    pub(crate) fn call_raw<Q>(
        &mut self,
        ordinal: u64,
        request: &Q,
    ) -> Result<(alloc::vec::Vec<u8>, alloc::vec::Vec<tee_fidl::HandleRef>), tee_fidl::FidlWireError>
    where
        Q: TeeEncode,
    {
        let mut request_bytes = alloc::vec![0; 65500];
        let mut request_handles = [tee_fidl::HandleRef { raw: 0 }; 8];
        let encoded = request.encode(&mut request_bytes, &mut request_handles)?;
        let message = bexos_userspace::Rpc(self.channel).call_raw(
            ordinal,
            &request_bytes[..encoded.bytes],
            &request_handles[..encoded.handles]
                .iter()
                .map(|h| h.raw)
                .collect::<alloc::vec::Vec<_>>(),
            true,
        );
        let message = match message {
            Ok(message) => message,
            Err(_) => return Err(tee_fidl::FidlWireError::Transport),
        };
        let response_handles = message
            .handles
            .iter()
            .map(|h| tee_fidl::HandleRef { raw: *h })
            .collect::<alloc::vec::Vec<_>>();
        Ok((message.bytes, response_handles))
    }
}

impl TeeManager for TeeServiceManager {
    async fn concurrent_storage_probe(&mut self) -> bexos_debug_wire::DebugStatusResponse {
        tee_probe::run(self).await
    }
    async fn info(&mut self) -> bexos_debug_wire::TeeInfoResponse {
        let Ok((bytes, handles)) = self.call_raw(1, &TeeManagerGetTeeInfoRequest {}) else {
            return bexos_debug_wire::TeeInfoResponse {
                status: -6,
                present: false,
                kind: "Unavailable".into(),
                secure_os_version: 0,
                anti_rollback_version: 0,
            };
        };
        match TeeManagerGetTeeInfoResponse::decode(&bytes, &handles) {
            Ok(response) => bexos_debug_wire::TeeInfoResponse {
                status: response.status as i32,
                present: response.present,
                kind: tee_kind_name(response.kind).into(),
                secure_os_version: response.secure_os_version,
                anti_rollback_version: response.anti_rollback_version,
            },
            Err(_) => bexos_debug_wire::TeeInfoResponse {
                status: -6,
                present: false,
                kind: "Unavailable".into(),
                secure_os_version: 0,
                anti_rollback_version: 0,
            },
        }
    }

    async fn list_apps(&mut self) -> bexos_debug_wire::TeeAppListResponse {
        let Ok((bytes, handles)) = self.call_raw(2, &TeeManagerListTrustedAppsRequest {}) else {
            return bexos_debug_wire::TeeAppListResponse {
                status: -6,
                apps: alloc::vec::Vec::new(),
            };
        };
        let Ok(response) = TeeManagerListTrustedAppsResponse::decode(&bytes, &handles) else {
            return bexos_debug_wire::TeeAppListResponse {
                status: -6,
                apps: alloc::vec::Vec::new(),
            };
        };
        let mut apps = alloc::vec::Vec::new();
        for index in 0..response.apps.len() {
            if let Ok(app) = response.apps.get(index) {
                apps.push(bexos_debug_wire::TeeAppInfo {
                    uuid: app.uuid.to_vec(),
                    version: app.version,
                    active_sessions: app.active_sessions,
                    entry_point_name: app.entry_point_name.into(),
                    storage_bytes_used: app.storage_bytes_used,
                    package_id: app.package_id.into(),
                    protected: app.protected,
                    package_managed: app.package_managed,
                });
            }
        }
        bexos_debug_wire::TeeAppListResponse {
            status: response.status as i32,
            apps,
        }
    }

    async fn install_app(
        &mut self,
        request: bexos_debug_wire::TeeInstallAppRequest,
    ) -> bexos_debug_wire::TeeInstallAppResponse {
        let Ok(vmo) = Memory::from_bytes(&request.payload) else {
            return bexos_debug_wire::TeeInstallAppResponse {
                status: -3,
                uuid: alloc::vec::Vec::new(),
            };
        };
        let response = self.call_raw(
            3,
            &TeeManagerInstallTrustedAppRequest {
                ta_payload: tee_fidl::HandleRef { raw: vmo },
                ta_payload_len: request.payload.len() as u64,
            },
        );
        let _ = Memory::close(vmo);
        let Ok((bytes, handles)) = response else {
            return bexos_debug_wire::TeeInstallAppResponse {
                status: -6,
                uuid: alloc::vec::Vec::new(),
            };
        };
        match TeeManagerInstallTrustedAppResponse::decode(&bytes, &handles) {
            Ok(response) => bexos_debug_wire::TeeInstallAppResponse {
                status: response.status as i32,
                uuid: response.uuid.to_vec(),
            },
            Err(_) => bexos_debug_wire::TeeInstallAppResponse {
                status: -8,
                uuid: alloc::vec::Vec::new(),
            },
        }
    }

    async fn uninstall_app(
        &mut self,
        request: bexos_debug_wire::TeeUuidRequest,
    ) -> bexos_debug_wire::DebugStatusResponse {
        let Some(uuid) = uuid16(&request.uuid) else {
            return debug_status(-8, "invalid tee UUID");
        };
        let response = self.call_raw(4, &TeeManagerUninstallTrustedAppRequest { uuid });
        tee_status_response(response, |bytes, handles| {
            TeeManagerUninstallTrustedAppResponse::decode(bytes, handles).map(|r| r.status as i32)
        })
    }

    async fn open_session(
        &mut self,
        request: bexos_debug_wire::TeeUuidRequest,
    ) -> bexos_debug_wire::TeeOpenSessionResponse {
        let Some(uuid) = uuid16(&request.uuid) else {
            return bexos_debug_wire::TeeOpenSessionResponse {
                status: -8,
                session_id: 0,
            };
        };
        let response = self.call_raw(5, &TeeManagerOpenSessionRequest { uuid });
        match response
            .and_then(|(bytes, handles)| TeeManagerOpenSessionResponse::decode(&bytes, &handles))
        {
            Ok(response) => bexos_debug_wire::TeeOpenSessionResponse {
                status: response.status as i32,
                session_id: response.session_id,
            },
            Err(_) => bexos_debug_wire::TeeOpenSessionResponse {
                status: -6,
                session_id: 0,
            },
        }
    }

    async fn close_session(
        &mut self,
        request: bexos_debug_wire::TeeSessionRequest,
    ) -> bexos_debug_wire::DebugStatusResponse {
        let response = self.call_raw(
            6,
            &TeeManagerCloseSessionRequest {
                session_id: request.session_id,
            },
        );
        tee_status_response(response, |bytes, handles| {
            TeeManagerCloseSessionResponse::decode(bytes, handles).map(|r| r.status as i32)
        })
    }

    async fn invoke(
        &mut self,
        request: bexos_debug_wire::TeeInvokeRequest,
    ) -> bexos_debug_wire::TeeInvokeResponse {
        let vmo = if request.payload.is_empty() {
            0
        } else {
            let Ok(vmo) = Memory::from_bytes(&request.payload) else {
                return bexos_debug_wire::TeeInvokeResponse {
                    status: -3,
                    response: alloc::vec::Vec::new(),
                };
            };
            vmo
        };
        let response = self.call_raw(
            7,
            &TeeManagerInvokeCommandRequest {
                session_id: request.session_id,
                command_id: request.command_id,
                payload: tee_fidl::HandleRef { raw: vmo },
                payload_len: request.payload.len() as u64,
            },
        );
        if vmo != 0 {
            let _ = Memory::close(vmo);
        }
        let Ok(response) = response
            .and_then(|(bytes, handles)| TeeManagerInvokeCommandResponse::decode(&bytes, &handles))
        else {
            return bexos_debug_wire::TeeInvokeResponse {
                status: -6,
                response: alloc::vec::Vec::new(),
            };
        };
        let bytes = read_vmo(response.response.raw, response.response_len).unwrap_or_default();
        let _ = Memory::close(response.response.raw);
        bexos_debug_wire::TeeInvokeResponse {
            status: response.status as i32,
            response: bytes,
        }
    }

    async fn update_core(
        &mut self,
        request: bexos_debug_wire::TeeUpdateCoreRequest,
    ) -> bexos_debug_wire::TeeUpdateCoreResponse {
        let Some(artifact_hash) = hash32(&request.artifact_hash) else {
            return bexos_debug_wire::TeeUpdateCoreResponse {
                status: -8,
                new_version: 0,
                message: "invalid tee artifact hash".into(),
            };
        };
        let Ok(vmo) = Memory::from_bytes(&request.image) else {
            return bexos_debug_wire::TeeUpdateCoreResponse {
                status: -3,
                new_version: 0,
                message: "tee image VMO failed".into(),
            };
        };
        let response = self.call_raw(
            8,
            &TeeManagerUpdateTeeCoreRequest {
                generation: request.generation,
                target: &request.target,
                activation: tee_fidl::TeeActivationMode::LiveNow,
                artifact_hash,
                tee_image: tee_fidl::HandleRef { raw: vmo },
                tee_image_len: request.image.len() as u64,
            },
        );
        let _ = Memory::close(vmo);
        match response {
            Ok((bytes, handles)) => match TeeManagerUpdateTeeCoreResponse::decode(&bytes, &handles)
            {
                Ok(response) => bexos_debug_wire::TeeUpdateCoreResponse {
                    status: response.status as i32,
                    new_version: response.new_version,
                    message: response.message.into(),
                },
                Err(_) => bexos_debug_wire::TeeUpdateCoreResponse {
                    status: -6,
                    new_version: 0,
                    message: "tee core update decode failed".into(),
                },
            },
            Err(_) => bexos_debug_wire::TeeUpdateCoreResponse {
                status: -6,
                new_version: 0,
                message: "tee core update transport failed".into(),
            },
        }
    }

    async fn update_status(&mut self) -> bexos_debug_wire::TeeUpdateStatusResponse {
        let Ok((bytes, handles)) = self.call_raw(9, &TeeManagerGetTeeUpdateStatusRequest {}) else {
            return bexos_debug_wire::TeeUpdateStatusResponse {
                status: -6,
                update_status: "Unavailable".into(),
                generation: 0,
                message: "tee status transport failed".into(),
                ..Default::default()
            };
        };
        match TeeManagerGetTeeUpdateStatusResponse::decode(&bytes, &handles) {
            Ok(response) => bexos_debug_wire::TeeUpdateStatusResponse {
                status: response.status as i32,
                update_status: tee_update_status_name(response.update_status).into(),
                generation: response.generation,
                message: response.message.into(),
                phase: alloc::format!("{:?}", response.phase),
                active_slot: response.active_slot.into(),
                pending_slot: response.pending_slot.into(),
                reboot_required: response.reboot_required,
                rollback_available: response.rollback_available,
            },
            Err(_) => bexos_debug_wire::TeeUpdateStatusResponse {
                status: -6,
                update_status: "Unavailable".into(),
                generation: 0,
                message: "tee status decode failed".into(),
                ..Default::default()
            },
        }
    }
}
