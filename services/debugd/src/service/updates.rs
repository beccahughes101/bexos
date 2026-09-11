use super::*;

pub(super) fn update_candidate_matches(
    selector_kind: u32,
    target: &str,
    candidate: &UpdateCandidateInfo,
) -> bool {
    match selector_kind {
        1 => true,
        2 => candidate.kind == 1 && candidate.target == target,
        3 => candidate.kind == 2,
        4 => candidate.kind == 3,
        5 => candidate.kind == 4,
        _ => false,
    }
}

pub trait UpdateManager {
    async fn check_updates(&mut self, _request: UpdateCheckRequest) -> UpdateCheckResponse {
        UpdateCheckResponse {
            status: unsupported_update_response().status,
            message: unsupported_update_response().message,
            candidates: Vec::new(),
        }
    }

    async fn stage_from_feed(&mut self, _request: UpdateCheckRequest) -> DebugStatusResponse {
        unsupported_update_response()
    }

    async fn apply_from_feed(&mut self, _request: UpdateCheckRequest) -> DebugStatusResponse {
        unsupported_update_response()
    }
    async fn apply_firmware(&mut self, _on_reboot: bool) -> DebugStatusResponse {
        unsupported_update_response()
    }
    async fn apply_firmware_from_feed(
        &mut self,
        _selector: u32,
        _on_reboot: bool,
    ) -> DebugStatusResponse {
        unsupported_update_response()
    }

    async fn apply_service<A: AppManager>(&mut self, _apps: &mut A) -> DebugStatusResponse {
        unsupported_update_response()
    }

    async fn begin_upload(
        &mut self,
        request: bexos_debug_wire::UpdateUploadBeginRequest,
    ) -> DebugStatusResponse;
    async fn write_chunk(
        &mut self,
        request: bexos_debug_wire::UpdateChunkRequest,
    ) -> DebugStatusResponse;
    async fn commit_upload(
        &mut self,
        request: bexos_debug_wire::UpdateUploadCommitRequest,
    ) -> DebugStatusResponse;
    async fn apply_app<A: AppManager>(&mut self, apps: &mut A) -> DebugStatusResponse;
    async fn apply_platform<P: PlatformUpdateApplier, T: TeeManager>(
        &mut self,
        platform: &mut P,
        tee: &mut T,
    ) -> DebugStatusResponse;
    async fn status(&mut self) -> DebugStatusResponse;
}

pub struct UnsupportedUpdateManager;

impl UpdateManager for UnsupportedUpdateManager {
    async fn begin_upload(
        &mut self,
        _request: bexos_debug_wire::UpdateUploadBeginRequest,
    ) -> DebugStatusResponse {
        unsupported_update_response()
    }

    async fn write_chunk(
        &mut self,
        _request: bexos_debug_wire::UpdateChunkRequest,
    ) -> DebugStatusResponse {
        unsupported_update_response()
    }

    async fn commit_upload(
        &mut self,
        _request: bexos_debug_wire::UpdateUploadCommitRequest,
    ) -> DebugStatusResponse {
        unsupported_update_response()
    }

    async fn apply_app<A: AppManager>(&mut self, _apps: &mut A) -> DebugStatusResponse {
        unsupported_update_response()
    }

    async fn apply_platform<P: PlatformUpdateApplier, T: TeeManager>(
        &mut self,
        _platform: &mut P,
        _tee: &mut T,
    ) -> DebugStatusResponse {
        unsupported_update_response()
    }

    async fn status(&mut self) -> DebugStatusResponse {
        unsupported_update_response()
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BufferedUpdateManager {
    pub(super) upload: Option<BufferedUpdateUpload>,
    pub(super) staged: Option<StagedUpdate>,
    pub(super) last_status: DebugStatusResponse,
    pub(super) minimum_generation: u64,
    pub(super) pending_platform_generation: Option<u64>,
    pub(super) feed_candidates: Vec<UpdateCandidateInfo>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct BufferedUpdateUpload {
    pub(super) upload_id: u64,
    pub(super) manifest_len: usize,
    pub(super) artifact_len: usize,
    pub(super) manifest: Vec<u8>,
    pub(super) artifact: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct StagedUpdate {
    pub(super) manifest: Vec<u8>,
    pub(super) artifact: Vec<u8>,
}

impl BufferedUpdateManager {
    pub fn new() -> Self {
        Self {
            last_status: ok_response("update idle"),
            ..Self::default()
        }
    }

    pub fn seed_feed_candidate(&mut self, candidate: UpdateCandidateInfo) {
        self.feed_candidates.push(candidate);
    }

    pub fn reconcile_platform<P: PlatformUpdateApplier>(&mut self, platform: &mut P) {
        let Some(pending) = self.pending_platform_generation else {
            return;
        };
        match platform.completion() {
            Some(Ok(generation)) if generation == pending => {
                self.minimum_generation = self.minimum_generation.max(generation.saturating_add(1));
                self.pending_platform_generation = None;
                self.last_status = ok_response("platform transplant completed");
            }
            Some(Err(())) => {
                self.pending_platform_generation = None;
                self.last_status =
                    debug_status(-6, "platform preparation aborted; old kernel still serving");
            }
            _ => {}
        }
    }

    fn verify(&self) -> Result<(UpdateManifest, Vec<u8>), UpdateError> {
        let staged = self.staged.as_ref().ok_or(UpdateError::UnexpectedEof)?;
        let verified = verify_update(
            &staged.manifest,
            &staged.artifact,
            &qemu_trusted_update_keys(),
            self.minimum_generation,
        )?;
        Ok((verified.manifest, staged.artifact.clone()))
    }
}

impl UpdateManager for BufferedUpdateManager {
    async fn check_updates(&mut self, request: UpdateCheckRequest) -> UpdateCheckResponse {
        let mut candidates: Vec<_> = self
            .feed_candidates
            .iter()
            .filter(|candidate| {
                update_candidate_matches(request.selector_kind, &request.target, candidate)
            })
            .cloned()
            .collect();
        if !request.all {
            candidates.truncate(1);
        }
        if request.stage {
            self.last_status = ok_response("update staged from TUF feed");
        }
        if request.apply {
            self.last_status = ok_response("update applied from TUF feed");
        }
        UpdateCheckResponse {
            status: 0,
            message: if candidates.is_empty() {
                "no updates available".into()
            } else {
                "updates available".into()
            },
            candidates,
        }
    }

    async fn stage_from_feed(&mut self, request: UpdateCheckRequest) -> DebugStatusResponse {
        let response = self.check_updates(request).await;
        if response.status == 0 && !response.candidates.is_empty() {
            ok_response("update staged from TUF feed")
        } else {
            debug_status(response.status, &response.message)
        }
    }

    async fn apply_from_feed(&mut self, request: UpdateCheckRequest) -> DebugStatusResponse {
        let response = self.check_updates(request).await;
        if response.status == 0 && !response.candidates.is_empty() {
            ok_response("update applied from TUF feed")
        } else {
            debug_status(response.status, &response.message)
        }
    }

    async fn begin_upload(
        &mut self,
        request: bexos_debug_wire::UpdateUploadBeginRequest,
    ) -> DebugStatusResponse {
        if request.manifest_len == 0
            || request.manifest_len > 64 * 1024
            || request.artifact_len == 0
            || request.artifact_len > 64 * 1024 * 1024
        {
            return invalid_request_response("invalid update upload dimensions");
        }
        self.upload = Some(BufferedUpdateUpload {
            upload_id: request.upload_id,
            manifest_len: request.manifest_len as usize,
            artifact_len: request.artifact_len as usize,
            manifest: Vec::new(),
            artifact: Vec::new(),
        });
        self.last_status = ok_response("update upload started");
        self.last_status.clone()
    }

    async fn write_chunk(
        &mut self,
        request: bexos_debug_wire::UpdateChunkRequest,
    ) -> DebugStatusResponse {
        let Some(upload) = self.upload.as_mut() else {
            return invalid_request_response("no active update upload");
        };
        if upload.upload_id != request.upload_id {
            return invalid_request_response("update upload id mismatch");
        }
        let (target, expected_len) = match request.stream {
            1 => (&mut upload.manifest, upload.manifest_len),
            2 => (&mut upload.artifact, upload.artifact_len),
            _ => return invalid_request_response("invalid update stream"),
        };
        if request.offset as usize != target.len() {
            return invalid_request_response("out-of-order update chunk");
        }
        if target.len().saturating_add(request.bytes.len()) > expected_len {
            return invalid_request_response("update chunk exceeds declared length");
        }
        target.extend_from_slice(&request.bytes);
        self.last_status = ok_response("update chunk accepted");
        self.last_status.clone()
    }

    async fn commit_upload(
        &mut self,
        request: bexos_debug_wire::UpdateUploadCommitRequest,
    ) -> DebugStatusResponse {
        let Some(upload) = self.upload.take() else {
            return invalid_request_response("no active update upload");
        };
        if upload.upload_id != request.upload_id {
            self.upload = Some(upload);
            return invalid_request_response("update upload id mismatch");
        }
        if upload.manifest.len() != upload.manifest_len
            || upload.artifact.len() != upload.artifact_len
        {
            self.upload = Some(upload);
            return invalid_request_response("update upload incomplete");
        }
        self.staged = Some(StagedUpdate {
            manifest: upload.manifest,
            artifact: upload.artifact,
        });
        self.last_status = ok_response("update staged");
        self.last_status.clone()
    }

    async fn apply_app<A: AppManager>(&mut self, apps: &mut A) -> DebugStatusResponse {
        let (manifest, artifact) = match self.verify() {
            Ok(update) => update,
            Err(error) => {
                return debug_status(-10, &alloc::format!("update verify failed: {error:?}"));
            }
        };
        if manifest.artifact_kind != ArtifactKind::AppPackage {
            return invalid_request_response("staged update is not an app package");
        }
        let upload_id = manifest.generation ^ artifact.len() as u64 ^ 0xbe05_0002;
        let begin = apps
            .begin_bundle_upload(bexos_debug_wire::AppBundleUploadBeginRequest {
                upload_id,
                archive_len: artifact.len() as u64,
            })
            .await;
        if begin.status != 0 {
            return begin;
        }
        let chunk = apps
            .write_bundle_chunk(bexos_debug_wire::AppBundleChunkRequest {
                upload_id,
                offset: 0,
                bytes: artifact,
            })
            .await;
        if chunk.status != 0 {
            return chunk;
        }
        let committed = apps
            .commit_bundle_upload(bexos_debug_wire::AppBundleUploadCommitRequest { upload_id })
            .await;
        if committed.status == 0 {
            self.minimum_generation = manifest.generation.saturating_add(1);
            self.last_status = debug_status(0, "app update applied");
            self.staged = None;
            self.last_status.clone()
        } else {
            committed
        }
    }

    async fn apply_service<A: AppManager>(&mut self, apps: &mut A) -> DebugStatusResponse {
        let (manifest, artifact) = match self.verify() {
            Ok(update) => update,
            Err(error) => {
                return debug_status(-10, &alloc::format!("update verify failed: {error:?}"));
            }
        };
        if manifest.artifact_kind != ArtifactKind::AppPackage {
            return invalid_request_response("service update requires app package");
        }
        let result = apps
            .migrate_service(manifest.generation, &manifest.target_id, &artifact)
            .await;
        if result.status == 0 {
            self.staged = None;
        }
        self.last_status = result.clone();
        result
    }

    async fn apply_platform<P: PlatformUpdateApplier, T: TeeManager>(
        &mut self,
        platform: &mut P,
        tee: &mut T,
    ) -> DebugStatusResponse {
        self.reconcile_platform(platform);
        if self.pending_platform_generation.is_some() {
            return debug_status(-8, "platform transplant already in progress");
        }
        let (manifest, artifact) = match self.verify() {
            Ok(update) => update,
            Err(error) => {
                return debug_status(-10, &alloc::format!("update verify failed: {error:?}"));
            }
        };
        if !matches!(
            manifest.artifact_kind,
            ArtifactKind::Microkernel | ArtifactKind::TeeImage | ArtifactKind::Hypervisor
        ) {
            return invalid_request_response("staged update is not a platform artifact");
        }
        if manifest.artifact_kind == ArtifactKind::TeeImage {
            let applied = tee
                .update_core(TeeUpdateCoreRequest {
                    generation: manifest.generation,
                    target: manifest.target_id.clone(),
                    artifact_hash: manifest.artifact_hash.to_vec(),
                    image: artifact,
                })
                .await;
            if applied.status == 0 {
                self.minimum_generation = manifest.generation.saturating_add(1);
                self.staged = None;
            }
            self.last_status = DebugStatusResponse {
                status: applied.status,
                message: applied.message,
            };
            return self.last_status.clone();
        }
        let applied = platform.apply_platform_update(&manifest, &artifact).await;
        if applied.status == 0 {
            self.pending_platform_generation = Some(manifest.generation);
            self.staged = None;
        }
        self.last_status = applied.clone();
        applied
    }

    async fn status(&mut self) -> DebugStatusResponse {
        self.last_status.clone()
    }
}
