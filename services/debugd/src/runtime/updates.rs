use super::*;

pub(crate) struct UpdateServiceManager {
    pub(crate) channel: Channel,
    pub(crate) awaiting_response: bool,
    pub(crate) upload: Option<ProxiedUpdateUpload>,
}

pub(crate) struct ProxiedUpdateUpload {
    pub(crate) upload_id: u64,
    pub(crate) manifest_len: usize,
    pub(crate) artifact_len: usize,
    pub(crate) manifest: alloc::vec::Vec<u8>,
    pub(crate) artifact: alloc::vec::Vec<u8>,
}

impl UpdateServiceManager {
    pub(crate) fn new(manager: Channel) -> Self {
        let channel = match Channel::pair() {
            Ok((client, server)) => {
                if manager
                    .send(
                        b"bexos.update.UpdateManager|UpdateManager|Public|1,2,3,4,5,6,7,8,9,10,11,12",
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
        Self {
            channel,
            awaiting_response: false,
            upload: None,
        }
    }

    pub(crate) fn call_raw<Q>(
        &mut self,
        ordinal: u64,
        request: &Q,
    ) -> Result<
        (alloc::vec::Vec<u8>, alloc::vec::Vec<update_fidl::HandleRef>),
        update_fidl::FidlWireError,
    >
    where
        Q: UpdateEncode,
    {
        let mut request_bytes = alloc::vec![0; 65500];
        let mut request_handles = [update_fidl::HandleRef { raw: 0 }; 8];
        let encoded = request.encode(&mut request_bytes, &mut request_handles)?;
        let message = bexos_userspace::Rpc(self.channel)
            .call_ordered_with_timeout(
                ordinal,
                &request_bytes[..encoded.bytes],
                &request_handles[..encoded.handles]
                    .iter()
                    .map(|h| h.raw)
                    .collect::<alloc::vec::Vec<_>>(),
                // App installation includes the nested durable VFS budget.
                // Secure replacement deadlines remain enforced by its owner.
                if matches!(ordinal, 4 | 9 | 10) {
                    420
                } else {
                    60
                },
                &mut self.awaiting_response,
            )
            .map_err(|_| update_fidl::FidlWireError::Transport)?;
        let response_handles = message
            .handles
            .iter()
            .map(|h| update_fidl::HandleRef { raw: *h })
            .collect::<alloc::vec::Vec<_>>();
        Ok((message.bytes, response_handles))
    }

    pub(crate) fn forward_upload(
        &mut self,
        upload: &ProxiedUpdateUpload,
    ) -> bexos_debug_wire::DebugStatusResponse {
        let begin = update_status_response(
            self.call_raw(
                1,
                &UpdateManagerBeginUploadRequest {
                    upload_id: upload.upload_id,
                    manifest_len: upload.manifest_len as u64,
                    artifact_len: upload.artifact_len as u64,
                },
            ),
            |bytes, handles| {
                UpdateManagerBeginUploadResponse::decode(bytes, handles)
                    .map(|r| (r.status, r.message.into()))
            },
        );
        if begin.status != 0 {
            return begin;
        }
        let manifest =
            self.forward_stream(upload.upload_id, UpdateStream::Manifest, &upload.manifest);
        if manifest.status != 0 {
            return manifest;
        }
        let artifact =
            self.forward_stream(upload.upload_id, UpdateStream::Artifact, &upload.artifact);
        if artifact.status != 0 {
            return artifact;
        }
        update_status_response(
            self.call_raw(
                3,
                &UpdateManagerCommitUploadRequest {
                    upload_id: upload.upload_id,
                },
            ),
            |bytes, handles| {
                UpdateManagerCommitUploadResponse::decode(bytes, handles)
                    .map(|r| (r.status, r.message.into()))
            },
        )
    }

    pub(crate) fn forward_stream(
        &mut self,
        upload_id: u64,
        stream: UpdateStream,
        bytes: &[u8],
    ) -> bexos_debug_wire::DebugStatusResponse {
        const CHUNK: usize = 48 * 1024;
        for (index, chunk) in bytes.chunks(CHUNK).enumerate() {
            let response = update_status_response(
                self.call_raw(
                    2,
                    &UpdateManagerWriteChunkRequest {
                        upload_id,
                        stream,
                        offset: (index * CHUNK) as u64,
                        bytes: chunk,
                    },
                ),
                |bytes, handles| {
                    UpdateManagerWriteChunkResponse::decode(bytes, handles)
                        .map(|r| (r.status, r.message.into()))
                },
            );
            if response.status != 0 {
                return response;
            }
        }
        debug_status(0, "update stream forwarded")
    }

    pub(crate) async fn begin_upload(
        &mut self,
        request: bexos_debug_wire::UpdateUploadBeginRequest,
    ) -> bexos_debug_wire::DebugStatusResponse {
        if request.manifest_len == 0
            || request.manifest_len > 64 * 1024
            || request.artifact_len == 0
            || request.artifact_len > 64 * 1024 * 1024
        {
            return debug_status(-8, "invalid update upload dimensions");
        }
        self.upload = Some(ProxiedUpdateUpload {
            upload_id: request.upload_id,
            manifest_len: request.manifest_len as usize,
            artifact_len: request.artifact_len as usize,
            manifest: alloc::vec::Vec::new(),
            artifact: alloc::vec::Vec::new(),
        });
        debug_status(0, "update upload started")
    }

    pub(crate) async fn check_updates(
        &mut self,
        request: UpdateCheckRequest,
    ) -> UpdateCheckResponse {
        let response = self.call_raw(
            8,
            &UpdateManagerCheckForUpdatesRequest {
                selector: UpdateSelector {
                    kind: fidl_selector_kind(request.selector_kind),
                    target: &request.target,
                },
                all: request.all,
            },
        );
        let Ok((bytes, handles)) = response else {
            return UpdateCheckResponse {
                status: -60,
                message: "updated transport failed".into(),
                candidates: Vec::new(),
            };
        };
        match UpdateManagerCheckForUpdatesResponse::decode(&bytes, &handles) {
            Ok(response) => match (0..response.candidates.len())
                .map(|index| {
                    response
                        .candidates
                        .get(index)
                        .map(|candidate| UpdateCandidateInfo {
                            target: candidate.target.into(),
                            path: candidate.path.into(),
                            url: candidate.url.into(),
                            kind: candidate.kind as u32,
                            generation: candidate.generation,
                            length: candidate.length,
                        })
                })
                .collect()
            {
                Ok(candidates) => UpdateCheckResponse {
                    status: response.status as i32,
                    message: response.message.into(),
                    candidates,
                },
                Err(_) => UpdateCheckResponse {
                    status: -60,
                    message: "updated decode failed".into(),
                    candidates: Vec::new(),
                },
            },
            Err(_) => UpdateCheckResponse {
                status: -60,
                message: "updated decode failed".into(),
                candidates: Vec::new(),
            },
        }
    }

    pub(crate) async fn stage_from_feed(
        &mut self,
        request: UpdateCheckRequest,
    ) -> bexos_debug_wire::DebugStatusResponse {
        update_status_response(
            self.call_raw(
                9,
                &UpdateManagerStageFromFeedRequest {
                    selector: UpdateSelector {
                        kind: fidl_selector_kind(request.selector_kind),
                        target: &request.target,
                    },
                },
            ),
            |bytes, handles| {
                UpdateManagerStageFromFeedResponse::decode(bytes, handles)
                    .map(|r| (r.status, r.message.into()))
            },
        )
    }

    pub(crate) async fn apply_from_feed(
        &mut self,
        request: UpdateCheckRequest,
    ) -> bexos_debug_wire::DebugStatusResponse {
        update_status_response(
            self.call_raw(
                10,
                &UpdateManagerApplyFromFeedRequest {
                    selector: UpdateSelector {
                        kind: fidl_selector_kind(request.selector_kind),
                        target: &request.target,
                    },
                },
            ),
            |bytes, handles| {
                UpdateManagerApplyFromFeedResponse::decode(bytes, handles)
                    .map(|r| (r.status, r.message.into()))
            },
        )
    }

    pub(crate) async fn write_chunk(
        &mut self,
        request: bexos_debug_wire::UpdateChunkRequest,
    ) -> bexos_debug_wire::DebugStatusResponse {
        let Some(upload) = self.upload.as_mut() else {
            return debug_status(-8, "no active update upload");
        };
        if upload.upload_id != request.upload_id {
            return debug_status(-8, "update upload id mismatch");
        }
        let (target, expected_len) = match request.stream {
            1 => (&mut upload.manifest, upload.manifest_len),
            2 => (&mut upload.artifact, upload.artifact_len),
            _ => return debug_status(-8, "invalid update stream"),
        };
        if request.offset as usize != target.len() {
            return debug_status(-8, "out-of-order update chunk");
        }
        if target.len().saturating_add(request.bytes.len()) > expected_len {
            return debug_status(-8, "update chunk exceeds declared length");
        }
        target.extend_from_slice(&request.bytes);
        debug_status(0, "update chunk accepted")
    }

    pub(crate) async fn commit_upload(
        &mut self,
        request: bexos_debug_wire::UpdateUploadCommitRequest,
    ) -> bexos_debug_wire::DebugStatusResponse {
        let Some(upload) = self.upload.take() else {
            return debug_status(-8, "no active update upload");
        };
        if upload.upload_id != request.upload_id {
            self.upload = Some(upload);
            return debug_status(-8, "update upload id mismatch");
        }
        if upload.manifest.len() != upload.manifest_len
            || upload.artifact.len() != upload.artifact_len
        {
            self.upload = Some(upload);
            return debug_status(-8, "update upload incomplete");
        }
        let response = self.forward_upload(&upload);
        if response.status != 0 {
            self.upload = Some(upload);
        }
        response
    }

    pub(crate) async fn apply_app(&mut self) -> bexos_debug_wire::DebugStatusResponse {
        update_status_response(
            self.call_raw(4, &UpdateManagerApplyAppRequest {}),
            |bytes, handles| {
                UpdateManagerApplyAppResponse::decode(bytes, handles)
                    .map(|r| (r.status, r.message.into()))
            },
        )
    }

    pub(crate) async fn apply_service(&mut self) -> bexos_debug_wire::DebugStatusResponse {
        update_status_response(
            self.call_raw(5, &UpdateManagerApplyServiceRequest {}),
            |bytes, handles| {
                UpdateManagerApplyServiceResponse::decode(bytes, handles)
                    .map(|r| (r.status, r.message.into()))
            },
        )
    }

    pub(crate) async fn apply_firmware_from_feed(
        &mut self,
        selector: u32,
        on_reboot: bool,
    ) -> bexos_debug_wire::DebugStatusResponse {
        update_status_response(
            self.call_raw(
                12,
                &update_fidl::UpdateManagerApplyFirmwareFromFeedRequest {
                    selector: UpdateSelector {
                        kind: fidl_selector_kind(selector),
                        target: "",
                    },
                    activation: if on_reboot {
                        update_fidl::FirmwareActivationMode::OnReboot
                    } else {
                        update_fidl::FirmwareActivationMode::LiveNow
                    },
                },
            ),
            |bytes, handles| {
                update_fidl::UpdateManagerApplyFirmwareFromFeedResponse::decode(bytes, handles)
                    .map(|r| (r.status, r.message.into()))
            },
        )
    }
    pub(crate) async fn apply_firmware(
        &mut self,
        on_reboot: bool,
    ) -> bexos_debug_wire::DebugStatusResponse {
        update_status_response(
            self.call_raw(
                11,
                &update_fidl::UpdateManagerApplyFirmwareRequest {
                    activation: if on_reboot {
                        update_fidl::FirmwareActivationMode::OnReboot
                    } else {
                        update_fidl::FirmwareActivationMode::LiveNow
                    },
                },
            ),
            |bytes, handles| {
                update_fidl::UpdateManagerApplyFirmwareResponse::decode(bytes, handles)
                    .map(|r| (r.status, r.message.into()))
            },
        )
    }
    pub(crate) async fn apply_platform(&mut self) -> bexos_debug_wire::DebugStatusResponse {
        update_status_response(
            self.call_raw(6, &UpdateManagerApplyPlatformRequest {}),
            |bytes, handles| {
                UpdateManagerApplyPlatformResponse::decode(bytes, handles)
                    .map(|r| (r.status, r.message.into()))
            },
        )
    }

    pub(crate) async fn status(&mut self) -> bexos_debug_wire::DebugStatusResponse {
        update_status_response(
            self.call_raw(7, &UpdateManagerGetStatusRequest {}),
            |bytes, handles| {
                UpdateManagerGetStatusResponse::decode(bytes, handles)
                    .map(|r| (r.status, r.message.into()))
            },
        )
    }
}

pub(crate) fn update_status_response(
    response: Result<
        (alloc::vec::Vec<u8>, alloc::vec::Vec<update_fidl::HandleRef>),
        update_fidl::FidlWireError,
    >,
    decode: impl FnOnce(
        &[u8],
        &[update_fidl::HandleRef],
    )
        -> Result<(UpdateStatus, alloc::string::String), update_fidl::FidlWireError>,
) -> bexos_debug_wire::DebugStatusResponse {
    let Ok((bytes, handles)) = response else {
        return debug_status(-60, "updated transport failed");
    };
    match decode(&bytes, &handles) {
        Ok((status, message)) => debug_status(status as i32, &message),
        Err(_) => debug_status(-60, "updated decode failed"),
    }
}

pub(crate) fn fidl_selector_kind(kind: u32) -> UpdateSelectorKind {
    match kind {
        2 => UpdateSelectorKind::Package,
        3 => UpdateSelectorKind::Kernel,
        4 => UpdateSelectorKind::Tee,
        5 => UpdateSelectorKind::Hypervisor,
        _ => UpdateSelectorKind::All,
    }
}
