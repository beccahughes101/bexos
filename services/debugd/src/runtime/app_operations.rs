use super::*;

impl AppManager for LifecycleAppManager {
    async fn terminate_selected_shell(
        &mut self,
        package: &str,
        uid: u64,
    ) -> bexos_debug_wire::DebugStatusResponse {
        let Ok((bytes, handles)) = self.call_raw(
            17,
            &lifecycle::AppLifecycleControlTerminateSelectedShellForDebugRequest {
                package_id: package,
                uid,
            },
        ) else {
            return debug_status(-6, "selected shell termination transport");
        };
        match lifecycle::AppLifecycleControlTerminateSelectedShellForDebugResponse::decode(
            &bytes, &handles,
        ) {
            Ok(q) => debug_status(q.status as i32, "selected shell termination requested"),
            Err(_) => debug_status(-6, "selected shell termination decode"),
        }
    }

    async fn preferences(
        &mut self,
        request: bexos_debug_wire::PreferencesRequest,
    ) -> bexos_debug_wire::PreferencesResponse {
        let mut input = Vec::new();
        bexos_debug_wire::encode_preferences_request(&request, &mut input);
        let result = self.call_raw_with_timeout(
            16,
            &lifecycle::AppLifecycleControlPreferencesRequest { request: &input },
            900_000,
        );
        match result {
            Ok((bytes, handles)) => {
                lifecycle::AppLifecycleControlPreferencesResponse::decode(&bytes, &handles)
                    .ok()
                    .and_then(|r| bexos_debug_wire::decode_preferences_response(r.response).ok())
                    .unwrap_or_else(|| bexos_debug_wire::PreferencesResponse {
                        status: -8,
                        message: "invalid preference response".into(),
                        ..Default::default()
                    })
            }
            Err(_) => bexos_debug_wire::PreferencesResponse {
                status: -20,
                message: "preference transport failed; read generation before retrying".into(),
                ..Default::default()
            },
        }
    }

    async fn process_progress(&mut self, package: &str) -> bexos_debug_wire::DebugStatusResponse {
        let Ok((bytes, handles)) = self.call_raw(
            7,
            &lifecycle::AppLifecycleControlGetProcessProgressRequest {
                package_id: package,
            },
        ) else {
            return debug_status(-6, "progress transport");
        };
        match lifecycle::AppLifecycleControlGetProcessProgressResponse::decode(&bytes, &handles) {
            Ok(q) => debug_status(
                q.status as i32,
                &alloc::format!("completed={} errors={}", q.completed, q.errors),
            ),
            Err(_) => debug_status(-6, "progress decode"),
        }
    }
    async fn migrate_service(
        &mut self,
        generation: u64,
        target: &str,
        artifact: &[u8],
    ) -> bexos_debug_wire::DebugStatusResponse {
        let Ok(handle) = Memory::from_bytes(artifact) else {
            return debug_status(-6, "archive allocation");
        };
        let result = self.call_raw(
            5,
            &lifecycle::AppLifecycleControlBeginMigrationRequest {
                archive: lifecycle::HandleRef { raw: handle },
                archive_len: artifact.len() as u64,
                generation,
                target,
            },
        );
        let Ok((bytes, handles)) = result else {
            let _ = Memory::close(handle);
            return debug_status(-6, "migration transport");
        };
        match lifecycle::AppLifecycleControlBeginMigrationResponse::decode(&bytes, &handles) {
            Ok(q) => debug_status(q.status as i32, q.message),
            Err(_) => debug_status(-6, "migration response"),
        }
    }
    async fn migrate_service_from_store(
        &mut self,
        archive_id: &str,
        generation: u64,
        target: &str,
    ) -> bexos_debug_wire::DebugStatusResponse {
        bexos_userspace::log("debugd: stored migration forwarding begin\n");
        let mut sent = false;
        let Ok((bytes, handles)) = self.call_raw_tracking_send(
            9,
            &lifecycle::AppLifecycleControlBeginMigrationFromStoredArchiveRequest {
                archive_id,
                generation,
                target,
            },
            900_000,
            &mut sent,
        ) else {
            if sent {
                bexos_userspace::log("debugd: stored migration forwarded without response\n");
            }
            return debug_status(-6, "stored migration transport");
        };
        bexos_userspace::log("debugd: stored migration response received\n");
        match lifecycle::AppLifecycleControlBeginMigrationFromStoredArchiveResponse::decode(
            &bytes, &handles,
        ) {
            Ok(q) => debug_status(q.status as i32, q.message),
            Err(_) => debug_status(-6, "stored migration response"),
        }
    }
    async fn migration_status(&mut self, target: &str) -> bexos_debug_wire::DebugStatusResponse {
        let Ok((bytes, handles)) = self.call_raw_with_timeout(
            6,
            &lifecycle::AppLifecycleControlGetMigrationStatusRequest { target },
            30_000,
        ) else {
            return debug_status(-6, "migration status transport");
        };
        match lifecycle::AppLifecycleControlGetMigrationStatusResponse::decode(&bytes, &handles) {
            Ok(q) => debug_status(
                q.status as i32,
                &alloc::format!(
                    "generation={} pending={} {}",
                    q.generation,
                    q.pending,
                    q.message
                ),
            ),
            Err(_) => debug_status(-6, "migration status decode"),
        }
    }

    async fn list_apps(
        &mut self,
    ) -> Result<alloc::vec::Vec<bexos_debug_wire::AppInfo>, bexos_debug_wire::DebugStatusResponse>
    {
        let response = self.call_raw(1, &lifecycle::AppLifecycleControlListAppsRequest {});
        let Ok((response_bytes, response_handles)) = response else {
            return Err(debug_status(-6, "app lifecycle list transport failed"));
        };
        let response = lifecycle::AppLifecycleControlListAppsResponse::decode(
            &response_bytes,
            &response_handles,
        );
        let Ok(response) = response else {
            return Err(debug_status(-6, "app lifecycle list decode failed"));
        };
        if response.status != lifecycle::AppLifecycleStatus::Ok {
            return Err(debug_status(
                response.status as i32,
                "app lifecycle list failed",
            ));
        }
        let mut apps = alloc::vec::Vec::new();
        for index in 0..response.apps.len() {
            let app = response
                .apps
                .get(index)
                .map_err(|_| debug_status(-6, "app lifecycle decode failed"))?;
            apps.push(bexos_debug_wire::AppInfo {
                package_id: app.package_id.into(),
                name: app.name.into(),
                state: alloc::format!("{:?}", app.state),
                source: alloc::format!("{:?}", app.source),
                protected: app.protected,
            });
        }
        Ok(apps)
    }

    async fn begin_bundle_upload(
        &mut self,
        request: bexos_debug_wire::AppBundleUploadBeginRequest,
    ) -> bexos_debug_wire::DebugStatusResponse {
        if request.archive_len == 0 || request.archive_len > 32 * 1024 * 1024 {
            return debug_status(-8, "invalid app bundle upload dimensions");
        }
        self.upload = Some(LifecycleUpload {
            upload_id: request.upload_id,
            archive_len: request.archive_len as usize,
            archive: alloc::vec::Vec::new(),
        });
        debug_status(0, "app bundle upload started")
    }

    async fn write_bundle_chunk(
        &mut self,
        request: bexos_debug_wire::AppBundleChunkRequest,
    ) -> bexos_debug_wire::DebugStatusResponse {
        let Some(upload) = self.upload.as_mut() else {
            return debug_status(-8, "no active app bundle upload");
        };
        if upload.upload_id != request.upload_id {
            return debug_status(-8, "app bundle upload id mismatch");
        }
        if request.offset as usize != upload.archive.len() {
            return debug_status(-8, "out-of-order app bundle chunk");
        }
        if upload.archive.len().saturating_add(request.bytes.len()) > upload.archive_len {
            return debug_status(-8, "app bundle chunk exceeds declared length");
        }
        upload.archive.extend_from_slice(&request.bytes);
        debug_status(0, "app bundle chunk accepted")
    }

    async fn commit_bundle_upload(
        &mut self,
        request: bexos_debug_wire::AppBundleUploadCommitRequest,
    ) -> bexos_debug_wire::DebugStatusResponse {
        let Some(upload) = self.upload.take() else {
            return debug_status(-8, "no active app bundle upload");
        };
        if upload.upload_id != request.upload_id {
            self.upload = Some(upload);
            return debug_status(-8, "app bundle upload id mismatch");
        }
        if upload.archive.len() != upload.archive_len {
            self.upload = Some(upload);
            return debug_status(-8, "app bundle upload incomplete");
        }
        let Ok(archive) = Memory::from_bytes(&upload.archive) else {
            return debug_status(-5, "app bundle VMO allocation failed");
        };
        let response = self.call_raw_with_timeout(
            2,
            &lifecycle::AppLifecycleControlInstallBundleRequest {
                archive: lifecycle::HandleRef { raw: archive },
                archive_len: upload.archive.len() as u64,
            },
            360_000,
        );
        let Ok((response_bytes, response_handles)) = response else {
            return debug_status(-6, "app lifecycle install transport failed");
        };
        let response = lifecycle::AppLifecycleControlInstallBundleResponse::decode(
            &response_bytes,
            &response_handles,
        );
        let Ok(response) = response else {
            return debug_status(-6, "app lifecycle install decode failed");
        };
        debug_status(response.status as i32, response.package_id)
    }

    async fn uninstall(
        &mut self,
        request: bexos_debug_wire::AppUninstallRequest,
    ) -> bexos_debug_wire::DebugStatusResponse {
        let response = self.call_raw(
            3,
            &lifecycle::AppLifecycleControlUninstallRequest {
                package_id: &request.package_id,
            },
        );
        match response {
            Ok((response_bytes, response_handles)) => {
                match lifecycle::AppLifecycleControlUninstallResponse::decode(
                    &response_bytes,
                    &response_handles,
                ) {
                    Ok(response) => debug_status(response.status as i32, "app uninstall complete"),
                    Err(_) => debug_status(-6, "app lifecycle uninstall decode failed"),
                }
            }
            Err(_) => debug_status(-6, "app lifecycle uninstall transport failed"),
        }
    }

    async fn launch(
        &mut self,
        request: bexos_debug_wire::AppLaunchRequest,
    ) -> bexos_debug_wire::DebugStatusResponse {
        let response = self.call_raw_with_timeout(
            4,
            &lifecycle::AppLifecycleControlLaunchRequest {
                package_id: &request.package_id,
                process_name: &request.process_name,
                arg0: request.arg0,
                uid: request.uid,
            },
            // A first launch may compile a previously unseen component on the
            // device. Keep this envelope alive after slower VFS work so its
            // eventual reply cannot be mistaken for a later lifecycle call.
            900_000,
        );
        match response {
            Ok((response_bytes, response_handles)) => {
                match lifecycle::AppLifecycleControlLaunchResponse::decode(
                    &response_bytes,
                    &response_handles,
                ) {
                    Ok(response) => debug_status(response.status as i32, "app launch complete"),
                    Err(_) => debug_status(-6, "app lifecycle launch decode failed"),
                }
            }
            Err(_) => debug_status(-6, "app lifecycle launch transport failed"),
        }
    }

    async fn get_component_config(
        &mut self,
        request: bexos_debug_wire::ComponentConfigGetRequest,
    ) -> bexos_debug_wire::ComponentConfigGetResponse {
        let Ok((bytes, handles)) = self.call_raw_with_timeout(
            10,
            &lifecycle::AppLifecycleControlGetComponentConfigRequest {
                package_id: &request.package_id,
            },
            360_000,
        ) else {
            return bexos_debug_wire::ComponentConfigGetResponse {
                status: -6,
                generation: 0,
                config: Vec::new(),
            };
        };
        match lifecycle::AppLifecycleControlGetComponentConfigResponse::decode(&bytes, &handles) {
            Ok(response) => {
                let config = read_vmo(response.config.raw, response.config_len);
                let _ = Memory::close(response.config.raw);
                bexos_debug_wire::ComponentConfigGetResponse {
                    status: if response.status == lifecycle::ComponentConfigStatus::Ok
                        && config.is_err()
                    {
                        -20
                    } else {
                        response.status as i32
                    },
                    generation: response.generation,
                    config: config.unwrap_or_default(),
                }
            }
            Err(_) => bexos_debug_wire::ComponentConfigGetResponse {
                status: -6,
                generation: 0,
                config: Vec::new(),
            },
        }
    }

    async fn set_component_config(
        &mut self,
        request: bexos_debug_wire::ComponentConfigSetRequest,
    ) -> bexos_debug_wire::ComponentConfigMutationResponse {
        let Ok(handle) = Memory::from_bytes(&request.config) else {
            return bexos_debug_wire::ComponentConfigMutationResponse {
                status: -5,
                generation: 0,
                message: "component config VMO allocation failed".into(),
            };
        };
        let mut sent = false;
        let result = self.call_raw_tracking_send(
            11,
            &lifecycle::AppLifecycleControlSetComponentConfigRequest {
                package_id: &request.package_id,
                expected_generation: request.expected_generation,
                config: lifecycle::HandleRef { raw: handle },
                config_len: request.config.len() as u64,
            },
            360_000,
            &mut sent,
        );
        let Ok((bytes, handles)) = result else {
            // A successful send consumes the VMO even if waiting for the reply fails.
            if !sent {
                let _ = Memory::close(handle);
            }
            return bexos_debug_wire::ComponentConfigMutationResponse {
                status: -6,
                generation: 0,
                message: "component config transport failed; read generation before retrying"
                    .into(),
            };
        };
        match lifecycle::AppLifecycleControlSetComponentConfigResponse::decode(&bytes, &handles) {
            Ok(response) => bexos_debug_wire::ComponentConfigMutationResponse {
                status: response.status as i32,
                generation: response.generation,
                message: response.message.into(),
            },
            Err(_) => bexos_debug_wire::ComponentConfigMutationResponse {
                status: -6,
                generation: 0,
                message: "component config response decode failed".into(),
            },
        }
    }

    async fn reset_component_config(
        &mut self,
        request: bexos_debug_wire::ComponentConfigResetRequest,
    ) -> bexos_debug_wire::ComponentConfigMutationResponse {
        let Ok((bytes, handles)) = self.call_raw_with_timeout(
            12,
            &lifecycle::AppLifecycleControlResetComponentConfigRequest {
                package_id: &request.package_id,
                expected_generation: request.expected_generation,
            },
            360_000,
        ) else {
            return bexos_debug_wire::ComponentConfigMutationResponse {
                status: -6,
                generation: 0,
                message: "component config transport failed; read generation before retrying"
                    .into(),
            };
        };
        match lifecycle::AppLifecycleControlResetComponentConfigResponse::decode(&bytes, &handles) {
            Ok(response) => bexos_debug_wire::ComponentConfigMutationResponse {
                status: response.status as i32,
                generation: response.generation,
                message: response.message.into(),
            },
            Err(_) => bexos_debug_wire::ComponentConfigMutationResponse {
                status: -6,
                generation: 0,
                message: "component config response decode failed".into(),
            },
        }
    }

    async fn install_from_url(
        &mut self,
        request: bexos_debug_wire::AppInstallFromUrlRequest,
    ) -> bexos_debug_wire::DebugStatusResponse {
        let response = self.call_app_manager(
            1,
            &app_manager::AppManagerInstallAppFromUrlRequest { url: &request.url },
        );
        match response {
            Ok((bytes, handles)) => {
                match app_manager::AppManagerInstallAppFromUrlResponse::decode(&bytes, &handles) {
                    Ok(response) => {
                        debug_status(response.status as i32, response.allocated_package_id)
                    }
                    Err(_) => debug_status(-6, "app manager install-url decode failed"),
                }
            }
            Err(_) => debug_status(-6, "app manager install-url transport failed"),
        }
    }

    async fn reload_well_known(
        &mut self,
        request: bexos_debug_wire::WellKnownReloadRequest,
    ) -> bexos_debug_wire::DebugStatusResponse {
        let response = self.call_app_manager(
            2,
            &app_manager::AppManagerReloadWellKnownForDomainRequest {
                domain: &request.domain,
            },
        );
        match response {
            Ok((bytes, handles)) => {
                match app_manager::AppManagerReloadWellKnownForDomainResponse::decode(
                    &bytes, &handles,
                ) {
                    Ok(response) => debug_status(
                        response.status as i32,
                        &alloc::format!(
                            "{:?} updated_handlers={}",
                            response.association,
                            response.updated_handlers_count
                        ),
                    ),
                    Err(_) => debug_status(-6, "app manager reload decode failed"),
                }
            }
            Err(_) => debug_status(-6, "app manager reload transport failed"),
        }
    }
}
