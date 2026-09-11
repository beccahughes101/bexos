use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use app_lifecycle_fidl as lifecycle;
use bexos_tuf::{MetadataSet, UpdateCandidate, UpdateSelector as TufUpdateSelector};
use bexos_update::{ArtifactKind, TrustedKey, UpdateManifest, verify_update};
use bexos_userspace::live_migration::Source;
use bexos_userspace::service_binding::{BoundServiceEndpoint, ServiceBinding};
use bexos_userspace::{Channel, KernelTransport, Memory, Rpc, Startup, log};
use kernel_fidl::{
    FidlDecode as KernelDecode, FidlEncode as KernelEncode, HandleRef,
    KernelDebugControlApplyPlatformUpdateRequest, KernelDebugControlGetUpdateStatusRequest,
    KernelDebugControlGetUpdateStatusResponse, KernelUpdateKind, Status as KernelStatus,
};
use lifecycle::{FidlDecode as LifecycleDecode, FidlEncode as LifecycleEncode};
use tee_fidl::{
    FidlDecode as TeeDecode, FidlEncode as TeeEncode, TeeManagerGetTeeUpdateStatusRequest,
    TeeManagerGetTeeUpdateStatusResponse, TeeManagerUpdateTeeCoreRequest,
    TeeManagerUpdateTeeCoreResponse,
};
use tee_manager_fidl as tee_fidl;
use update_manager_fidl::{
    FidlDecode, FirmwareActivationMode, UpdateCandidate as FidlUpdateCandidate,
    UpdateManagerApplyAppRequest, UpdateManagerApplyAppResponse,
    UpdateManagerApplyFirmwareFromFeedRequest, UpdateManagerApplyFirmwareFromFeedResponse,
    UpdateManagerApplyFirmwareRequest, UpdateManagerApplyFirmwareResponse,
    UpdateManagerApplyFromFeedRequest, UpdateManagerApplyFromFeedResponse,
    UpdateManagerApplyPlatformRequest, UpdateManagerApplyPlatformResponse,
    UpdateManagerApplyServiceRequest, UpdateManagerApplyServiceResponse,
    UpdateManagerBeginUploadRequest, UpdateManagerBeginUploadResponse,
    UpdateManagerCheckForUpdatesRequest, UpdateManagerCheckForUpdatesResponse,
    UpdateManagerCommitUploadRequest, UpdateManagerCommitUploadResponse,
    UpdateManagerGetStatusRequest, UpdateManagerGetStatusResponse,
    UpdateManagerStageFromFeedRequest, UpdateManagerStageFromFeedResponse,
    UpdateManagerWriteChunkRequest, UpdateManagerWriteChunkResponse, UpdateSelectorKind,
    UpdateStatus, UpdateStream, UpdateTargetKind, WireVector,
};

use crate::wire::{envelope, handle_refs, send_response};

const MAX_MANIFEST_BYTES: usize = 64 * 1024;
const MAX_ARTIFACT_BYTES: usize = 64 * 1024 * 1024;

pub async fn main(channel: u64) -> ! {
    let control = Channel(channel);
    let startup = Startup::receive(control).expect("updated startup");
    let tee = startup
        .service_grants
        .iter()
        .find(|grant| grant.service == "tee_manager")
        .map(|grant| Channel(grant.endpoint));
    if startup.migration_target {
        match bexos_userspace::live_migration::receive::<Runtime>(
            control,
            startup.migration_generation,
        ) {
            Ok(mut state) => {
                if state.tee.is_none() {
                    state.tee = tee;
                }
                serve(state).await
            }
            Err(_) => bexos_userspace::exit(),
        }
    }
    Startup::ready(control).unwrap();
    log("updated: ready\n");
    serve(Runtime {
        control,
        migration: startup.migration,
        manager: UpdateService::new(),
        clients: Vec::new(),
        appd: None,
        appd_awaiting_response: false,
        app_manager: None,
        tee,
        platform: PlatformUpdateApplier::new(),
        feed: FeedUpdateManager::new(),
    })
    .await
}

pub struct Runtime {
    pub control: Channel,
    pub migration: Option<Channel>,
    pub manager: UpdateService,
    pub clients: Vec<BoundServiceEndpoint>,
    pub appd: Option<Channel>,
    pub appd_awaiting_response: bool,
    pub app_manager: Option<Channel>,
    pub tee: Option<Channel>,
    pub platform: PlatformUpdateApplier,
    pub feed: FeedUpdateManager,
}

async fn serve(mut runtime: Runtime) -> ! {
    let mut source = Source::new(runtime.migration);
    loop {
        if source.poll(&runtime).is_err() {
            let _ = bexos_userspace::migration::abort();
        }
        if source.quiescing() {
            bexos_userspace::yield_now();
            continue;
        }
        let awaiting = runtime.platform.awaiting_completion;
        let appd_awaiting = runtime.appd_awaiting_response;
        runtime.platform.observe_completion();
        let completed_platform = runtime.manager.reconcile_platform(&mut runtime.platform);
        if let (Some(channel), Some(generation)) = (runtime.appd, completed_platform) {
            let mut appd = AppManager::new(channel, &mut runtime.appd_awaiting_response);
            let committed = appd.commit_generation_floor("kernel", generation.saturating_add(1));
            if committed.status == UpdateStatus::Ok {
                runtime.manager.minimum_generation = runtime
                    .manager
                    .minimum_generation
                    .max(committed_floor(&committed));
            }
        }
        if let Some(channel) = runtime.appd {
            let mut appd = AppManager::new(channel, &mut runtime.appd_awaiting_response);
            if runtime.manager.reconcile_service(&mut appd) {
                source.changed_keys([0, crate::migration::UPDATES]);
            }
        }
        if awaiting {
            source.changed_keys([0, crate::migration::UPDATES]);
        }
        if let Ok(message) = runtime.control.try_recv() {
            if message.bytes == b"bexos.updated.handoff.v1" {
                if let Some(handle) = message.handles.first() {
                    runtime.appd = Some(Channel(*handle));
                    runtime.appd_awaiting_response = false;
                    runtime.app_manager = message.handles.get(1).copied().map(Channel);
                    // Acknowledge ownership before making a synchronous call
                    // back into appd. appd cannot serve that callback until it
                    // receives this ack and enters its lifecycle loop.
                    let _ = runtime.control.send(b"bexos.updated.handoff.ok", &[]);
                    let mut appd =
                        AppManager::new(Channel(*handle), &mut runtime.appd_awaiting_response);
                    runtime.manager.restore_minimum_generation(&mut appd);
                    source.changed(0);
                    log("updated: app lifecycle proxy connected\n");
                } else {
                    let _ = runtime.control.send(b"bexos.updated.handoff.ok", &[]);
                }
            } else if let (Some(endpoint), Ok(metadata)) = (
                message.handles.first().copied(),
                core::str::from_utf8(&message.bytes),
            ) {
                if let Some(binding) = ServiceBinding::parse(metadata) {
                    if binding.protocol_is("UpdateManager") {
                        runtime.clients.push(BoundServiceEndpoint::new(
                            Channel(endpoint),
                            binding.method_ordinals,
                        ));
                        source.changed(0);
                    }
                } else {
                    let _ = Memory::close(endpoint);
                }
            }
        }
        if poll_clients(&mut runtime).await {
            source.changed_keys([0, crate::migration::UPDATES]);
        }
        if appd_awaiting != runtime.appd_awaiting_response {
            source.changed(0);
        }
        runtime.platform.commit_if_pending();
        bexos_userspace::yield_now();
    }
}

async fn poll_clients(runtime: &mut Runtime) -> bool {
    let mut changed = false;
    let mut clients = core::mem::take(&mut runtime.clients);
    clients.retain(|client| match client.channel.try_recv() {
        Ok(message) => {
            changed = true;
            let (ordinal, req) = envelope(&message.bytes);
            log("updated: client request\n");
            let handles = handle_refs(&message.handles);
            if client.allows(ordinal) {
                futures_dispatch(client.channel, ordinal, req, &handles, runtime);
            } else {
                send_invalid(client.channel, ordinal);
            }
            true
        }
        Err(KernelStatus::ErrPeerClosed) => {
            changed = true;
            false
        }
        Err(_) => true,
    });
    runtime.clients = clients;
    changed
}

fn futures_dispatch(
    channel: Channel,
    ordinal: u64,
    req: &[u8],
    handles: &[update_manager_fidl::HandleRef],
    runtime: &mut Runtime,
) {
    let response = match ordinal {
        1 => match UpdateManagerBeginUploadRequest::decode(req, handles) {
            Ok(request) => runtime.manager.begin_upload(request),
            Err(_) => StatusMessage::invalid("invalid BeginUpload request"),
        },
        2 => match UpdateManagerWriteChunkRequest::decode(req, handles) {
            Ok(request) => runtime.manager.write_chunk(request),
            Err(_) => StatusMessage::invalid("invalid WriteChunk request"),
        },
        3 => match UpdateManagerCommitUploadRequest::decode(req, handles) {
            Ok(request) => runtime.manager.commit_upload(request),
            Err(_) => StatusMessage::invalid("invalid CommitUpload request"),
        },
        4 => {
            let _ = UpdateManagerApplyAppRequest::decode(req, handles);
            runtime.apply_app()
        }
        5 => {
            let _ = UpdateManagerApplyServiceRequest::decode(req, handles);
            runtime.apply_service()
        }
        6 => {
            let _ = UpdateManagerApplyPlatformRequest::decode(req, handles);
            runtime.apply_platform()
        }
        7 => {
            let _ = UpdateManagerGetStatusRequest::decode(req, handles);
            send_response(channel, &runtime.manager.status_response());
            return;
        }
        8 => match UpdateManagerCheckForUpdatesRequest::decode(req, handles) {
            Ok(request) => {
                let response = runtime.check_for_updates(request);
                send_response(channel, &response);
                return;
            }
            Err(_) => StatusMessage::invalid("invalid CheckForUpdates request"),
        },
        9 => match UpdateManagerStageFromFeedRequest::decode(req, handles) {
            Ok(request) => runtime.stage_from_feed(request),
            Err(_) => StatusMessage::invalid("invalid StageFromFeed request"),
        },
        10 => match UpdateManagerApplyFromFeedRequest::decode(req, handles) {
            Ok(request) => runtime.apply_from_feed(request),
            Err(_) => StatusMessage::invalid("invalid ApplyFromFeed request"),
        },
        11 => match UpdateManagerApplyFirmwareRequest::decode(req, handles) {
            Ok(request) => runtime.apply_platform_mode(Some(firmware_mode(request.activation))),
            Err(_) => StatusMessage::invalid("invalid ApplyFirmware request"),
        },
        12 => match UpdateManagerApplyFirmwareFromFeedRequest::decode(req, handles) {
            Ok(request) => runtime.apply_from_feed_mode(
                UpdateManagerApplyFromFeedRequest {
                    selector: request.selector,
                },
                Some(firmware_mode(request.activation)),
            ),
            Err(_) => StatusMessage::invalid("invalid ApplyFirmwareFromFeed request"),
        },
        _ => StatusMessage::invalid("unknown update method"),
    };
    send_status(channel, ordinal, response);
}

fn send_status(channel: Channel, ordinal: u64, response: StatusMessage) {
    match ordinal {
        1 => send_response(
            channel,
            &UpdateManagerBeginUploadResponse {
                status: response.status,
                message: &response.message,
            },
        ),
        2 => send_response(
            channel,
            &UpdateManagerWriteChunkResponse {
                status: response.status,
                message: &response.message,
            },
        ),
        3 => send_response(
            channel,
            &UpdateManagerCommitUploadResponse {
                status: response.status,
                message: &response.message,
            },
        ),
        4 => send_response(
            channel,
            &UpdateManagerApplyAppResponse {
                status: response.status,
                message: &response.message,
            },
        ),
        5 => send_response(
            channel,
            &UpdateManagerApplyServiceResponse {
                status: response.status,
                message: &response.message,
            },
        ),
        6 => send_response(
            channel,
            &UpdateManagerApplyPlatformResponse {
                status: response.status,
                message: &response.message,
            },
        ),
        9 => send_response(
            channel,
            &UpdateManagerStageFromFeedResponse {
                status: response.status,
                message: &response.message,
            },
        ),
        10 => send_response(
            channel,
            &UpdateManagerApplyFromFeedResponse {
                status: response.status,
                message: &response.message,
            },
        ),
        11 => send_response(
            channel,
            &UpdateManagerApplyFirmwareResponse {
                status: response.status,
                message: &response.message,
            },
        ),
        12 => send_response(
            channel,
            &UpdateManagerApplyFirmwareFromFeedResponse {
                status: response.status,
                message: &response.message,
            },
        ),
        _ => {}
    }
}

fn firmware_mode(mode: FirmwareActivationMode) -> tee_fidl::TeeActivationMode {
    match mode {
        FirmwareActivationMode::LiveNow => tee_fidl::TeeActivationMode::LiveNow,
        FirmwareActivationMode::OnReboot => tee_fidl::TeeActivationMode::OnReboot,
    }
}
fn default_firmware_mode(kind: ArtifactKind) -> tee_fidl::TeeActivationMode {
    if kind == ArtifactKind::TeeImage {
        tee_fidl::TeeActivationMode::OnReboot
    } else {
        tee_fidl::TeeActivationMode::LiveNow
    }
}

fn send_invalid(channel: Channel, ordinal: u64) {
    send_status(
        channel,
        ordinal,
        StatusMessage::invalid("invalid update method"),
    );
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StatusMessage {
    pub status: UpdateStatus,
    pub message: String,
}

impl StatusMessage {
    fn ok(message: &str) -> Self {
        Self {
            status: UpdateStatus::Ok,
            message: message.into(),
        }
    }

    fn invalid(message: &str) -> Self {
        Self {
            status: UpdateStatus::ErrInvalidArgs,
            message: message.into(),
        }
    }

    fn verify(message: String) -> Self {
        Self {
            status: UpdateStatus::ErrVerifyFailed,
            message,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpdateService {
    pub(crate) upload: Option<Upload>,
    pub(crate) staged: Option<Staged>,
    pub(crate) last_status: i32,
    pub(crate) last_message: String,
    pub(crate) minimum_generation: u64,
    pub(crate) pending_platform_generation: Option<u64>,
    pub(crate) pending_service_generation: Option<PendingServiceGeneration>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PendingServiceGeneration {
    pub target: String,
    pub generation: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Upload {
    pub upload_id: u64,
    pub manifest_len: usize,
    pub artifact_len: usize,
    pub manifest: Vec<u8>,
    pub artifact: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Staged {
    pub manifest: Vec<u8>,
    pub artifact: Vec<u8>,
}

impl Default for UpdateService {
    fn default() -> Self {
        Self::new()
    }
}

impl UpdateService {
    pub fn new() -> Self {
        Self {
            upload: None,
            staged: None,
            last_status: 0,
            last_message: "update idle".into(),
            minimum_generation: 0,
            pending_platform_generation: None,
            pending_service_generation: None,
        }
    }

    pub fn begin_upload(&mut self, request: UpdateManagerBeginUploadRequest) -> StatusMessage {
        if request.manifest_len == 0
            || request.manifest_len > MAX_MANIFEST_BYTES as u64
            || request.artifact_len == 0
            || request.artifact_len > MAX_ARTIFACT_BYTES as u64
        {
            return self.record(StatusMessage::invalid("invalid update upload dimensions"));
        }
        self.upload = Some(Upload {
            upload_id: request.upload_id,
            manifest_len: request.manifest_len as usize,
            artifact_len: request.artifact_len as usize,
            manifest: Vec::new(),
            artifact: Vec::new(),
        });
        self.record(StatusMessage::ok("update upload started"))
    }

    pub fn write_chunk(&mut self, request: UpdateManagerWriteChunkRequest) -> StatusMessage {
        let Some(upload) = self.upload.as_mut() else {
            return self.record(StatusMessage::invalid("no active update upload"));
        };
        if upload.upload_id != request.upload_id {
            return self.record(StatusMessage::invalid("update upload id mismatch"));
        }
        let (target, expected_len) = match request.stream {
            UpdateStream::Manifest => (&mut upload.manifest, upload.manifest_len),
            UpdateStream::Artifact => (&mut upload.artifact, upload.artifact_len),
        };
        if request.offset as usize != target.len() {
            return self.record(StatusMessage::invalid("out-of-order update chunk"));
        }
        if target.len().saturating_add(request.bytes.len()) > expected_len {
            return self.record(StatusMessage::invalid(
                "update chunk exceeds declared length",
            ));
        }
        target.extend_from_slice(&request.bytes);
        self.record(StatusMessage::ok("update chunk accepted"))
    }

    pub fn commit_upload(&mut self, request: UpdateManagerCommitUploadRequest) -> StatusMessage {
        let Some(upload) = self.upload.take() else {
            return self.record(StatusMessage::invalid("no active update upload"));
        };
        if upload.upload_id != request.upload_id {
            self.upload = Some(upload);
            return self.record(StatusMessage::invalid("update upload id mismatch"));
        }
        if upload.manifest.len() != upload.manifest_len
            || upload.artifact.len() != upload.artifact_len
        {
            self.upload = Some(upload);
            return self.record(StatusMessage::invalid("update upload incomplete"));
        }
        self.staged = Some(Staged {
            manifest: upload.manifest,
            artifact: upload.artifact,
        });
        self.record(StatusMessage::ok("update staged"))
    }

    fn verify(&self) -> Result<(UpdateManifest, Vec<u8>), bexos_update::UpdateError> {
        let staged = self
            .staged
            .as_ref()
            .ok_or(bexos_update::UpdateError::UnexpectedEof)?;
        let verified = verify_update(
            &staged.manifest,
            &staged.artifact,
            &qemu_trusted_update_keys(),
            if matches!(
                bexos_update::parse_manifest(&staged.manifest)?.artifact_kind,
                ArtifactKind::TeeImage | ArtifactKind::Hypervisor
            ) {
                0
            } else {
                self.minimum_generation
            },
        )?;
        Ok((verified.manifest, staged.artifact.clone()))
    }

    fn record(&mut self, response: StatusMessage) -> StatusMessage {
        self.last_status = response.status as i32;
        self.last_message = response.message.clone();
        response
    }

    pub fn reconcile_platform(&mut self, platform: &mut PlatformUpdateApplier) -> Option<u64> {
        let Some(pending) = self.pending_platform_generation else {
            return None;
        };
        match platform.completion {
            Some(Ok(generation)) if generation == pending => {
                self.minimum_generation = self.minimum_generation.max(generation.saturating_add(1));
                self.pending_platform_generation = None;
                self.record(StatusMessage::ok("platform transplant completed"));
                Some(generation)
            }
            Some(Err(())) => {
                self.pending_platform_generation = None;
                self.record(StatusMessage {
                    status: UpdateStatus::ErrTransport,
                    message: "platform preparation aborted; old kernel still serving".into(),
                });
                None
            }
            _ => None,
        }
    }

    fn reconcile_service(&mut self, appd: &mut AppManager<'_>) -> bool {
        let Some(pending) = self.pending_service_generation.clone() else {
            return false;
        };
        let status = appd.migration_status(&pending.target);
        if status.status != UpdateStatus::Ok {
            return false;
        }
        let mut parts = status.message.split(':');
        let running = parts
            .next()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);
        let still_pending = parts.next() == Some("pending");
        if still_pending || running < pending.generation {
            return false;
        }
        let committed = appd.commit_generation_floor(&pending.target, pending.generation);
        if committed.status == UpdateStatus::Ok {
            self.minimum_generation = self
                .minimum_generation
                .max(committed_floor(&committed).saturating_add(1));
            self.pending_service_generation = None;
            self.staged = None;
            self.record(StatusMessage::ok("service transplant completed"));
            true
        } else {
            false
        }
    }

    fn restore_minimum_generation(&mut self, appd: &mut AppManager<'_>) {
        for target in ["kernel", "tee"] {
            let floor = appd.get_generation_floor(target);
            if floor.status == UpdateStatus::Ok {
                self.minimum_generation = self.minimum_generation.max(committed_floor(&floor));
            }
        }
    }

    pub fn status_response(&self) -> UpdateManagerGetStatusResponse<'_> {
        UpdateManagerGetStatusResponse {
            status: status_from_i32(self.last_status),
            message: &self.last_message,
            minimum_generation: self.minimum_generation,
            pending_platform_generation: self.pending_platform_generation.unwrap_or(0),
            has_upload: self.upload.is_some(),
            has_staged: self.staged.is_some(),
        }
    }
}

impl Runtime {
    fn check_for_updates<'a>(
        &'a mut self,
        request: UpdateManagerCheckForUpdatesRequest<'_>,
    ) -> UpdateManagerCheckForUpdatesResponse<'a> {
        let selector = tuf_selector(request.selector.kind, request.selector.target);
        let candidates =
            match self
                .feed
                .check(selector, request.all, self.manager.minimum_generation)
            {
                Ok(candidates) => candidates,
                Err(error) => {
                    self.manager.record(StatusMessage::verify(alloc::format!(
                        "tuf verify failed: {error:?}"
                    )));
                    return UpdateManagerCheckForUpdatesResponse {
                        status: UpdateStatus::ErrVerifyFailed,
                        message: &self.manager.last_message,
                        candidates: WireVector::from_slice(&[]),
                    };
                }
            };
        let message = if candidates.is_empty() {
            "no updates available"
        } else {
            "updates available"
        };
        self.manager.record(StatusMessage::ok(message));
        let fidl = self.feed.fidl_candidates();
        UpdateManagerCheckForUpdatesResponse {
            status: UpdateStatus::Ok,
            message: &self.manager.last_message,
            candidates: WireVector::from_slice(fidl),
        }
    }

    fn stage_from_feed(&mut self, request: UpdateManagerStageFromFeedRequest<'_>) -> StatusMessage {
        let selector = tuf_selector(request.selector.kind, request.selector.target);
        let candidates = match self
            .feed
            .check(selector, false, self.manager.minimum_generation)
        {
            Ok(candidates) => candidates,
            Err(error) => {
                return self.manager.record(StatusMessage::verify(alloc::format!(
                    "tuf verify failed: {error:?}"
                )));
            }
        };
        let Some(candidate) = candidates.first() else {
            return self.manager.record(StatusMessage {
                status: UpdateStatus::ErrNotFound,
                message: "no matching update target".into(),
            });
        };
        let Some(artifact) = self.feed.artifact(candidate).map(<[u8]>::to_vec) else {
            return self.manager.record(StatusMessage {
                status: UpdateStatus::ErrTransport,
                message: "update target artifact unavailable".into(),
            });
        };
        if let Err(error) = candidate.verify_target(&artifact) {
            return self.manager.record(StatusMessage::verify(alloc::format!(
                "target verify failed: {error:?}"
            )));
        }
        self.feed.staged = Some((candidate.clone(), artifact));
        self.manager
            .record(StatusMessage::ok("update staged from TUF feed"))
    }

    fn apply_from_feed(&mut self, request: UpdateManagerApplyFromFeedRequest<'_>) -> StatusMessage {
        self.apply_from_feed_mode(request, None)
    }
    fn apply_from_feed_mode(
        &mut self,
        request: UpdateManagerApplyFromFeedRequest<'_>,
        mode: Option<tee_fidl::TeeActivationMode>,
    ) -> StatusMessage {
        if mode.is_some()
            && !matches!(
                request.selector.kind,
                UpdateSelectorKind::Tee | UpdateSelectorKind::Hypervisor
            )
        {
            return self.manager.record(StatusMessage::invalid(
                "firmware activation requires a tee or hypervisor selector",
            ));
        }
        let staged = self.stage_from_feed(UpdateManagerStageFromFeedRequest {
            selector: request.selector,
        });
        if staged.status != UpdateStatus::Ok {
            return staged;
        }
        let Some((candidate, artifact)) = self.feed.staged.clone() else {
            return self
                .manager
                .record(StatusMessage::invalid("no staged feed update"));
        };
        match candidate.kind {
            ArtifactKind::AppPackage => {
                let Some(channel) = self.appd else {
                    return self.manager.record(StatusMessage {
                        status: UpdateStatus::ErrUnavailable,
                        message: "appd unavailable".into(),
                    });
                };
                let mut appd = AppManager::new(channel, &mut self.appd_awaiting_response);
                let result = appd.install_bundle(&artifact);
                if result.status == UpdateStatus::Ok {
                    self.manager.minimum_generation = candidate.generation.saturating_add(1);
                    self.feed.staged = None;
                }
                self.manager.record(result)
            }
            ArtifactKind::Microkernel | ArtifactKind::TeeImage | ArtifactKind::Hypervisor => {
                let manifest = UpdateManifest {
                    generation: candidate.generation,
                    target_id: candidate.target_id,
                    artifact_kind: candidate.kind,
                    artifact_len: artifact.len() as u64,
                    artifact_hash: blake3::hash(&artifact).into(),
                    key_id: [0; 32],
                    signature: [0; 64],
                };
                if matches!(
                    manifest.artifact_kind,
                    ArtifactKind::TeeImage | ArtifactKind::Hypervisor
                ) {
                    let Some(channel) = self.tee else {
                        return self.manager.record(StatusMessage {
                            status: UpdateStatus::ErrUnavailable,
                            message: "tee manager unavailable".into(),
                        });
                    };
                    log("updated: applying tee core update\n");
                    let mut tee = TeeManager { channel };
                    let activation =
                        mode.unwrap_or_else(|| default_firmware_mode(manifest.artifact_kind));
                    let result = tee.update_core(&manifest, &artifact, activation);
                    log("updated: tee core update returned\n");

                    if result.status == UpdateStatus::Ok {
                        self.feed.staged = None;
                    }
                    return self.manager.record(result);
                }
                let result = self.platform.apply_platform_update(&manifest, &artifact);
                if result.status == UpdateStatus::Ok {
                    self.manager.pending_platform_generation = Some(manifest.generation);
                    self.feed.staged = None;
                }
                self.manager.record(result)
            }
        }
    }

    fn apply_app(&mut self) -> StatusMessage {
        let (manifest, artifact) = match self.manager.verify() {
            Ok(update) => update,
            Err(error) => {
                return self.manager.record(StatusMessage::verify(alloc::format!(
                    "update verify failed: {error:?}"
                )));
            }
        };
        if manifest.artifact_kind != ArtifactKind::AppPackage {
            return self.manager.record(StatusMessage::invalid(
                "staged update is not an app package",
            ));
        }
        let Some(channel) = self.appd else {
            return self.manager.record(StatusMessage {
                status: UpdateStatus::ErrUnavailable,
                message: "appd unavailable".into(),
            });
        };
        let mut appd = AppManager::new(channel, &mut self.appd_awaiting_response);
        let result = appd.install_bundle(&artifact);
        if result.status == UpdateStatus::Ok {
            self.manager.minimum_generation = manifest.generation.saturating_add(1);
            self.manager.staged = None;
        }
        self.manager.record(result)
    }

    fn apply_service(&mut self) -> StatusMessage {
        let (manifest, artifact) = match self.manager.verify() {
            Ok(update) => update,
            Err(error) => {
                return self.manager.record(StatusMessage::verify(alloc::format!(
                    "update verify failed: {error:?}"
                )));
            }
        };
        if manifest.artifact_kind != ArtifactKind::AppPackage {
            return self.manager.record(StatusMessage::invalid(
                "service update requires app package",
            ));
        }
        let Some(channel) = self.appd else {
            return self.manager.record(StatusMessage {
                status: UpdateStatus::ErrUnavailable,
                message: "appd unavailable".into(),
            });
        };
        let mut appd = AppManager::new(channel, &mut self.appd_awaiting_response);
        let result = appd.migrate_service(manifest.generation, &manifest.target_id, &artifact);
        if result.status == UpdateStatus::Ok {
            self.manager.pending_service_generation = Some(PendingServiceGeneration {
                target: manifest.target_id.clone(),
                generation: manifest.generation,
            });
            return self
                .manager
                .record(StatusMessage::ok("service transplant started"));
        }
        self.manager.record(result)
    }

    fn apply_platform(&mut self) -> StatusMessage {
        self.apply_platform_mode(None)
    }
    fn apply_platform_mode(&mut self, mode: Option<tee_fidl::TeeActivationMode>) -> StatusMessage {
        self.manager.reconcile_platform(&mut self.platform);
        if self.manager.pending_platform_generation.is_some() {
            return self.manager.record(StatusMessage {
                status: UpdateStatus::ErrAlreadyExists,
                message: "platform transplant already in progress".into(),
            });
        }
        let (manifest, artifact) = match self.manager.verify() {
            Ok(update) => update,
            Err(error) => {
                return self.manager.record(StatusMessage::verify(alloc::format!(
                    "update verify failed: {error:?}"
                )));
            }
        };
        if mode.is_some()
            && !matches!(
                manifest.artifact_kind,
                ArtifactKind::TeeImage | ArtifactKind::Hypervisor
            )
        {
            return self.manager.record(StatusMessage::invalid(
                "firmware activation requires a tee or hypervisor artifact",
            ));
        }
        if !matches!(
            manifest.artifact_kind,
            ArtifactKind::Microkernel | ArtifactKind::TeeImage | ArtifactKind::Hypervisor
        ) {
            return self.manager.record(StatusMessage::invalid(
                "staged update is not a platform artifact",
            ));
        }
        if matches!(
            manifest.artifact_kind,
            ArtifactKind::TeeImage | ArtifactKind::Hypervisor
        ) {
            let Some(channel) = self.tee else {
                return self.manager.record(StatusMessage {
                    status: UpdateStatus::ErrUnavailable,
                    message: "tee manager unavailable".into(),
                });
            };
            log("updated: applying tee core update\n");
            let mut tee = TeeManager { channel };
            let activation = mode.unwrap_or_else(|| default_firmware_mode(manifest.artifact_kind));
            let result = tee.update_core(&manifest, &artifact, activation);
            log("updated: tee core update returned\n");

            if result.status == UpdateStatus::Ok {
                self.manager.staged = None;
            }
            return self.manager.record(result);
        }
        let result = self.platform.apply_platform_update(&manifest, &artifact);
        if result.status == UpdateStatus::Ok {
            self.manager.pending_platform_generation = Some(manifest.generation);
            self.manager.staged = None;
        }
        self.manager.record(result)
    }
}

struct AppManager<'a> {
    channel: Channel,
    awaiting_response: &'a mut bool,
}

impl<'a> AppManager<'a> {
    fn new(channel: Channel, awaiting_response: &'a mut bool) -> Self {
        Self {
            channel,
            awaiting_response,
        }
    }

    fn call_raw<Q>(
        &mut self,
        ordinal: u64,
        request: &Q,
    ) -> Result<(Vec<u8>, Vec<lifecycle::HandleRef>), lifecycle::FidlWireError>
    where
        Q: LifecycleEncode,
    {
        let mut request_bytes = alloc::vec![0; 65500];
        let mut request_handles = [lifecycle::HandleRef { raw: 0 }; 8];
        let encoded = request.encode(&mut request_bytes, &mut request_handles)?;
        let message = Rpc(self.channel)
            .call_ordered_with_timeout(
                ordinal,
                &request_bytes[..encoded.bytes],
                &request_handles[..encoded.handles]
                    .iter()
                    .map(|h| h.raw)
                    .collect::<Vec<_>>(),
                if matches!(ordinal, 2 | 14) { 360 } else { 60 },
                self.awaiting_response,
            )
            .map_err(|_| lifecycle::FidlWireError::Transport)?;
        let handles = message
            .handles
            .iter()
            .map(|h| lifecycle::HandleRef { raw: *h })
            .collect::<Vec<_>>();
        Ok((message.bytes, handles))
    }

    fn install_bundle(&mut self, artifact: &[u8]) -> StatusMessage {
        let Ok(archive) = Memory::from_bytes(artifact) else {
            return transport("app update archive VMO failed");
        };
        let response = self.call_raw(
            2,
            &lifecycle::AppLifecycleControlInstallBundleRequest {
                archive: lifecycle::HandleRef { raw: archive },
                archive_len: artifact.len() as u64,
            },
        );
        let _ = Memory::close(archive);
        status_response(response, |bytes, handles| {
            lifecycle::AppLifecycleControlInstallBundleResponse::decode(bytes, handles)
                .map(|r| (r.status as i32, "app update applied".to_string()))
        })
    }

    fn migrate_service(&mut self, generation: u64, target: &str, artifact: &[u8]) -> StatusMessage {
        let Ok(archive) = Memory::from_bytes(artifact) else {
            return transport("service update archive VMO failed");
        };
        let response = self.call_raw(
            5,
            &lifecycle::AppLifecycleControlBeginMigrationRequest {
                archive: lifecycle::HandleRef { raw: archive },
                archive_len: artifact.len() as u64,
                generation,
                target,
            },
        );
        let _ = Memory::close(archive);
        status_response(response, |bytes, handles| {
            lifecycle::AppLifecycleControlBeginMigrationResponse::decode(bytes, handles)
                .map(|r| (r.status as i32, r.message.to_string()))
        })
    }

    fn migration_status(&mut self, target: &str) -> StatusMessage {
        status_response(
            self.call_raw(
                6,
                &lifecycle::AppLifecycleControlGetMigrationStatusRequest { target },
            ),
            |bytes, handles| {
                lifecycle::AppLifecycleControlGetMigrationStatusResponse::decode(bytes, handles)
                    .map(|r| {
                        (
                            r.status as i32,
                            alloc::format!(
                                "{}:{}",
                                r.generation,
                                if r.pending { "pending" } else { "idle" }
                            ),
                        )
                    })
            },
        )
    }

    fn get_generation_floor(&mut self, target: &str) -> StatusMessage {
        status_response(
            self.call_raw(
                13,
                &lifecycle::AppLifecycleControlGetGenerationFloorRequest { target },
            ),
            |bytes, handles| {
                lifecycle::AppLifecycleControlGetGenerationFloorResponse::decode(bytes, handles)
                    .map(|r| (r.status as i32, r.generation.to_string()))
            },
        )
    }

    fn commit_generation_floor(&mut self, target: &str, generation: u64) -> StatusMessage {
        status_response(
            self.call_raw(
                14,
                &lifecycle::AppLifecycleControlCommitGenerationFloorRequest { target, generation },
            ),
            |bytes, handles| {
                lifecycle::AppLifecycleControlCommitGenerationFloorResponse::decode(bytes, handles)
                    .map(|r| (r.status as i32, r.generation.to_string()))
            },
        )
    }
}

fn committed_floor(status: &StatusMessage) -> u64 {
    status.message.parse().unwrap_or(0)
}

#[derive(Clone, Debug, Default)]
pub struct FeedUpdateManager {
    metadata: Option<MetadataSet>,
    trusted_root: Vec<u8>,
    client_state: Option<bexos_tuf::ClientState>,
    target_artifacts: BTreeMap<String, Vec<u8>>,
    staged: Option<(UpdateCandidate, Vec<u8>)>,
    fidl_scratch: Vec<FidlUpdateCandidate<'static>>,
}

impl FeedUpdateManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn seed_repository(&mut self, trusted_root: Vec<u8>, metadata: MetadataSet) {
        self.trusted_root = trusted_root;
        self.client_state = None;
        self.metadata = Some(metadata);
    }

    pub fn seed_artifact(&mut self, path_or_url: &str, bytes: Vec<u8>) {
        self.target_artifacts.insert(path_or_url.into(), bytes);
    }

    pub fn checkpoint(&self) -> Vec<u8> {
        let mut out = Vec::new();
        push_feed_bytes(&mut out, &self.trusted_root);
        if let Some(state) = &self.client_state {
            push_feed_bytes(&mut out, &state.encode_state());
        } else {
            push_feed_bytes(&mut out, &[]);
        }
        out
    }

    pub fn adopt_checkpoint(&mut self, bytes: &[u8]) -> Result<(), ()> {
        let mut cursor = FeedCursor { bytes, offset: 0 };
        self.trusted_root = cursor.bytes(1024 * 1024)?.to_vec();
        let state = cursor.bytes(2 * 1024 * 1024)?;
        self.client_state = if state.is_empty() {
            None
        } else {
            Some(bexos_tuf::ClientState::decode_state(state).map_err(|_| ())?)
        };
        if cursor.offset != bytes.len() {
            return Err(());
        }
        Ok(())
    }

    fn check(
        &mut self,
        selector: TufUpdateSelector<'_>,
        all: bool,
        minimum_generation: u64,
    ) -> Result<Vec<UpdateCandidate>, bexos_tuf::TufError> {
        let Some(metadata) = self.metadata.as_ref() else {
            self.fidl_scratch.clear();
            return Ok(Vec::new());
        };
        let mut state = match self.client_state.take() {
            Some(state) => state,
            None => bexos_tuf::ClientState::new(self.trusted_root.clone())?,
        };
        let repo = state.refresh(metadata, 0)?;
        self.trusted_root = state.root.bytes.clone();
        self.client_state = Some(state);
        let mut candidates: Vec<_> = repo
            .candidates(selector)
            .into_iter()
            .filter(|candidate| {
                matches!(
                    candidate.kind,
                    ArtifactKind::TeeImage | ArtifactKind::Hypervisor
                ) || candidate.generation >= minimum_generation
            })
            .collect();
        if !all {
            candidates.truncate(1);
        }
        self.set_fidl_candidates(&candidates);
        Ok(candidates)
    }

    fn artifact(&self, candidate: &UpdateCandidate) -> Option<&[u8]> {
        self.target_artifacts
            .get(&candidate.path)
            .or_else(|| {
                candidate
                    .url
                    .as_ref()
                    .and_then(|url| self.target_artifacts.get(url))
            })
            .map(Vec::as_slice)
    }

    fn fidl_candidates(&self) -> &[FidlUpdateCandidate<'static>] {
        &self.fidl_scratch
    }

    fn set_fidl_candidates(&mut self, candidates: &[UpdateCandidate]) {
        self.fidl_scratch.clear();
        self.fidl_scratch
            .extend(candidates.iter().map(|candidate| FidlUpdateCandidate {
                target: leak_string(candidate.target_id.clone()),
                path: leak_string(candidate.path.clone()),
                url: leak_string(candidate.url.clone().unwrap_or_default()),
                kind: fidl_target_kind(candidate.kind),
                generation: candidate.generation,
                length: candidate.length,
            }));
    }
}

fn push_feed_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
    out.extend_from_slice(bytes);
}

struct FeedCursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> FeedCursor<'a> {
    fn bytes(&mut self, max: usize) -> Result<&'a [u8], ()> {
        let len = self.u64()? as usize;
        if len > max {
            return Err(());
        }
        let end = self.offset.checked_add(len).ok_or(())?;
        let bytes = self.bytes.get(self.offset..end).ok_or(())?;
        self.offset = end;
        Ok(bytes)
    }

    fn u64(&mut self) -> Result<u64, ()> {
        let end = self.offset.checked_add(8).ok_or(())?;
        let bytes = self.bytes.get(self.offset..end).ok_or(())?;
        let mut raw = [0; 8];
        raw.copy_from_slice(bytes);
        self.offset = end;
        Ok(u64::from_le_bytes(raw))
    }
}

fn tuf_selector(kind: UpdateSelectorKind, target: &str) -> TufUpdateSelector<'_> {
    match kind {
        UpdateSelectorKind::Package => TufUpdateSelector::Package(target),
        UpdateSelectorKind::Kernel => TufUpdateSelector::Kernel,
        UpdateSelectorKind::Tee => TufUpdateSelector::Tee,
        UpdateSelectorKind::Hypervisor => TufUpdateSelector::Hypervisor,
        _ => TufUpdateSelector::All,
    }
}

fn fidl_target_kind(kind: ArtifactKind) -> UpdateTargetKind {
    match kind {
        ArtifactKind::AppPackage => UpdateTargetKind::AppPackage,
        ArtifactKind::Microkernel => UpdateTargetKind::Microkernel,
        ArtifactKind::TeeImage => UpdateTargetKind::TeeImage,
        ArtifactKind::Hypervisor => UpdateTargetKind::Hypervisor,
    }
}

fn leak_string(value: String) -> &'static str {
    alloc::boxed::Box::leak(value.into_boxed_str())
}

pub struct PlatformUpdateApplier {
    transport: KernelTransport,
    pub(crate) commit_pending: bool,
    pub(crate) awaiting_completion: bool,
    pub(crate) completion: Option<Result<u64, ()>>,
}

impl PlatformUpdateApplier {
    pub fn new() -> Self {
        Self {
            transport: KernelTransport(6),
            commit_pending: false,
            awaiting_completion: false,
            completion: None,
        }
    }

    fn commit_if_pending(&mut self) {
        if self.commit_pending {
            self.commit_pending = false;
            log("updated: committing staged kernel transplant\n");
            bexos_userspace::syscall::commit_transplant();
            self.awaiting_completion = true;
        }
    }

    fn observe_completion(&mut self) {
        if !self.awaiting_completion {
            return;
        }
        if let Ok((bytes, handles)) = self.call_raw(
            "GetUpdateStatus",
            &KernelDebugControlGetUpdateStatusRequest {},
        ) {
            if let Ok(status) = KernelDebugControlGetUpdateStatusResponse::decode(&bytes, &handles)
            {
                if status.update_status == kernel_fidl::KernelUpdateStatus::Completed {
                    self.completion = Some(Ok(status.generation));
                    self.awaiting_completion = false;
                    log("updated: resumed after kernel transplant\n");
                } else if status.update_status == kernel_fidl::KernelUpdateStatus::Failed {
                    self.completion = Some(Err(()));
                    self.awaiting_completion = false;
                    log("updated: kernel preparation failed; old kernel still serving\n");
                }
            }
        }
    }

    fn apply_platform_update(
        &mut self,
        manifest: &UpdateManifest,
        artifact: &[u8],
    ) -> StatusMessage {
        let kind = match manifest.artifact_kind {
            ArtifactKind::Microkernel => KernelUpdateKind::Microkernel,
            ArtifactKind::TeeImage => KernelUpdateKind::TeeImage,
            ArtifactKind::Hypervisor => KernelUpdateKind::Hypervisor,
            ArtifactKind::AppPackage => {
                return StatusMessage::invalid("app updates do not use kernel platform path");
            }
        };
        let Ok(artifact_vmo) = Memory::from_bytes(artifact) else {
            return transport("kernel platform update artifact VMO failed");
        };
        let response = self.call_raw(
            "ApplyPlatformUpdate",
            &KernelDebugControlApplyPlatformUpdateRequest {
                kind,
                generation: manifest.generation,
                target: &manifest.target_id,
                artifact_hash: &manifest.artifact_hash,
                artifact_len: manifest.artifact_len,
                artifact: HandleRef { raw: artifact_vmo },
            },
        );
        let _ = Memory::close(artifact_vmo);
        let Ok((bytes, handles)) = response else {
            return transport("kernel platform update transport failed");
        };
        let Ok(response) =
            kernel_fidl::KernelDebugControlApplyPlatformUpdateResponse::decode(&bytes, &handles)
        else {
            return transport("kernel platform update decode failed");
        };
        if response.status == KernelStatus::Ok {
            self.completion = None;
            self.commit_pending = true;
        }
        StatusMessage {
            status: status_from_i32(response.status as i32),
            message: response.message.to_string(),
        }
    }

    fn call_raw<Q>(
        &mut self,
        method_name: &str,
        request: &Q,
    ) -> Result<(Vec<u8>, Vec<HandleRef>), kernel_fidl::FidlWireError>
    where
        Q: KernelEncode,
    {
        let ordinal = kernel_fidl::KERNEL_DEBUG_CONTROL_PUBLIC_METHODS
            .iter()
            .find(|method| method.name == method_name)
            .map(|method| method.ordinal)
            .ok_or(kernel_fidl::FidlWireError::Transport)?;
        let mut request_bytes = alloc::vec![0; 1024];
        let mut request_handles = [HandleRef { raw: 0 }; 1];
        let encoded = request.encode(&mut request_bytes, &mut request_handles)?;
        let mut response_bytes = alloc::vec![0; 1024];
        let mut response_handles = [HandleRef { raw: 0 }; 4];
        let received = kernel_fidl::FidlTransport::call(
            &mut self.transport,
            ordinal,
            &request_bytes[..encoded.bytes],
            &request_handles[..encoded.handles],
            &mut response_bytes,
            &mut response_handles,
        )?;
        Ok((
            response_bytes[..received.bytes].to_vec(),
            response_handles[..received.handles].to_vec(),
        ))
    }
}

struct TeeManager {
    channel: Channel,
}

impl TeeManager {
    fn call_raw<Q>(
        &mut self,
        ordinal: u64,
        request: &Q,
    ) -> Result<(Vec<u8>, Vec<tee_fidl::HandleRef>), tee_fidl::FidlWireError>
    where
        Q: TeeEncode,
    {
        let mut request_bytes = alloc::vec![0; 65500];
        let mut request_handles = [tee_fidl::HandleRef { raw: 0 }; 8];
        let encoded = request.encode(&mut request_bytes, &mut request_handles)?;
        let message = Rpc(self.channel)
            .call_raw(
                ordinal,
                &request_bytes[..encoded.bytes],
                &request_handles[..encoded.handles]
                    .iter()
                    .map(|h| h.raw)
                    .collect::<Vec<_>>(),
                true,
            )
            .map_err(|_| tee_fidl::FidlWireError::Transport)?;
        let handles = message
            .handles
            .iter()
            .map(|h| tee_fidl::HandleRef { raw: *h })
            .collect::<Vec<_>>();
        Ok((message.bytes, handles))
    }

    fn update_core(
        &mut self,
        manifest: &UpdateManifest,
        artifact: &[u8],
        activation: tee_fidl::TeeActivationMode,
    ) -> StatusMessage {
        let Ok(tee_image) = Memory::from_bytes(artifact) else {
            return transport("tee update image VMO failed");
        };
        let response = self.call_raw(
            8,
            &TeeManagerUpdateTeeCoreRequest {
                generation: manifest.generation,
                target: &manifest.target_id,
                activation,
                artifact_hash: manifest.artifact_hash,
                tee_image: tee_fidl::HandleRef { raw: tee_image },
                tee_image_len: artifact.len() as u64,
            },
        );
        let _ = Memory::close(tee_image);
        status_response(response, |bytes, handles| {
            TeeManagerUpdateTeeCoreResponse::decode(bytes, handles).map(|r| {
                let message = if r.status == tee_fidl::TeeStatus::Ok {
                    r.message.to_string()
                } else {
                    alloc::format!("{:?}: {}", r.status, r.message)
                };
                (r.status as i32, message)
            })
        })
    }

    #[allow(dead_code)]
    fn update_status(&mut self) -> StatusMessage {
        status_response(
            self.call_raw(9, &TeeManagerGetTeeUpdateStatusRequest {}),
            |bytes, handles| {
                TeeManagerGetTeeUpdateStatusResponse::decode(bytes, handles)
                    .map(|r| (r.status as i32, r.message.to_string()))
            },
        )
    }
}

fn status_response<E, D>(
    response: Result<(Vec<u8>, Vec<E>), impl core::fmt::Debug>,
    decode: impl FnOnce(&[u8], &[E]) -> Result<(i32, String), D>,
) -> StatusMessage {
    let Ok((bytes, handles)) = response else {
        return transport("update transport failed");
    };
    let Ok((status, message)) = decode(&bytes, &handles) else {
        return transport("update decode failed");
    };
    StatusMessage {
        status: status_from_i32(status),
        message,
    }
}

fn transport(message: &str) -> StatusMessage {
    StatusMessage {
        status: UpdateStatus::ErrTransport,
        message: message.into(),
    }
}

fn status_from_i32(status: i32) -> UpdateStatus {
    match status {
        0 => UpdateStatus::Ok,
        -1 => UpdateStatus::ErrNotFound,
        -2 => UpdateStatus::ErrAccessDenied,
        -7 => UpdateStatus::ErrAlreadyExists,
        -8 => UpdateStatus::ErrInvalidArgs,
        -21 => UpdateStatus::ErrVerifyFailed,
        -95 => UpdateStatus::ErrUnavailable,
        _ => UpdateStatus::ErrTransport,
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
