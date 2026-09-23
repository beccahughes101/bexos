#![no_main]

extern crate alloc;
mod shell_runtime;
mod tee_probe;

use app_lifecycle_fidl as lifecycle;
use app_manager::{FidlDecode as AppManagerDecode, FidlEncode as AppManagerEncode};
use app_manager_fidl as app_manager;
use bexos_userspace::{Channel, KernelTransport, Memory, Startup, log};
use kernel_fidl::{
    FidlDecode as KernelDecode, FidlEncode as KernelEncode, HandleRef,
    KernelDebugControlApplyPlatformUpdateRequest, KernelDebugControlGetUpdateStatusRequest,
    KernelDebugControlGetUpdateStatusResponse, KernelDebugControlPublicClient, KernelUpdateKind,
    Status,
};
use lifecycle::{FidlDecode as LifecycleDecode, FidlEncode as LifecycleEncode};
use tee_fidl::{
    FidlDecode as TeeDecode, FidlEncode as TeeEncode, TeeManagerCloseSessionRequest,
    TeeManagerCloseSessionResponse, TeeManagerGetTeeInfoRequest, TeeManagerGetTeeInfoResponse,
    TeeManagerGetTeeUpdateStatusRequest, TeeManagerGetTeeUpdateStatusResponse,
    TeeManagerInstallTrustedAppRequest, TeeManagerInstallTrustedAppResponse,
    TeeManagerInvokeCommandRequest, TeeManagerInvokeCommandResponse,
    TeeManagerListTrustedAppsRequest, TeeManagerListTrustedAppsResponse,
    TeeManagerOpenSessionRequest, TeeManagerOpenSessionResponse,
    TeeManagerUninstallTrustedAppRequest, TeeManagerUninstallTrustedAppResponse,
    TeeManagerUpdateTeeCoreRequest, TeeManagerUpdateTeeCoreResponse,
};
use tee_manager_fidl as tee_fidl;
use update_manager_fidl as update_fidl;
use user_manager::{FidlDecode as UserDecode, FidlEncode as UserEncode};
use user_manager_fidl as user_manager;

use bexos_debug_wire::{UpdateCandidateInfo, UpdateCheckRequest, UpdateCheckResponse, parse_frame};
use bexos_debugd::transport::{ByteTransport, write_frame_async};
use bexos_debugd::{
    AppManager, BufferedTestAppInstaller, LiveTraceManager, PlatformUpdateApplier, TeeManager,
    TraceManager, TracedTraceManager, UnsupportedAppManager, UnsupportedTeeManager,
    UnsupportedTraceManager, UnsupportedUpdateManager, UnsupportedUserManager, UserManager,
    handle_frame_with_trace_backends,
};
use bexos_update::{ArtifactKind, UpdateManifest};
use update_fidl::{
    FidlDecode as UpdateDecode, FidlEncode as UpdateEncode, UpdateManagerApplyAppRequest,
    UpdateManagerApplyAppResponse, UpdateManagerApplyFromFeedRequest,
    UpdateManagerApplyFromFeedResponse, UpdateManagerApplyPlatformRequest,
    UpdateManagerApplyPlatformResponse, UpdateManagerApplyServiceRequest,
    UpdateManagerApplyServiceResponse, UpdateManagerBeginUploadRequest,
    UpdateManagerBeginUploadResponse, UpdateManagerCheckForUpdatesRequest,
    UpdateManagerCheckForUpdatesResponse, UpdateManagerCommitUploadRequest,
    UpdateManagerCommitUploadResponse, UpdateManagerGetStatusRequest,
    UpdateManagerGetStatusResponse, UpdateManagerStageFromFeedRequest,
    UpdateManagerStageFromFeedResponse, UpdateManagerWriteChunkRequest,
    UpdateManagerWriteChunkResponse, UpdateSelector, UpdateSelectorKind, UpdateStatus,
    UpdateStream,
};

bexos_libc::entry!(run);

#[path = "live_migration.rs"]
mod migration;
fn run(channel: u64) -> ! {
    bexos_userspace::block_on(main(channel))
}

async fn main(channel: u64) -> ! {
    use bexos_userspace::live_migration::State;
    let channel = Channel(channel);
    let startup = Startup::receive(channel).unwrap();
    if bexos_crypto::init_from_startup(&startup).is_err() {
        log("debugd: crypto library linker data missing\n");
        bexos_userspace::exit();
    }
    let state = if startup.migration_target {
        match bexos_userspace::live_migration::receive::<migration::Runtime>(
            channel,
            startup.migration_generation,
        ) {
            Ok(s) => s,
            Err(_) => bexos_userspace::exit(),
        }
    } else {
        let (h, va) = (
            *startup.resources.first().expect("virtio serial endpoint"),
            0,
        );

        Startup::ready(channel).unwrap();
        log("debugd: QEMU socket transport ready\n");
        let mut s = migration::Runtime::empty();
        s.control = channel;
        s.migration = startup.migration;
        s.uart_handle = h;
        s.uart_va = va;
        if let Some(grant) = startup
            .service_grants
            .iter()
            .find(|grant| grant.service == "tee_manager")
        {
            s.tee = LiveTeeManager::Proxy(TeeServiceManager::new(Channel(grant.endpoint)));
            log("debugd: tee proxy connected\n");
        }
        if let Some(grant) = startup
            .service_grants
            .iter()
            .find(|grant| grant.service == "bexos.tracing.TraceController")
        {
            s.traces = bexos_debugd::LiveTraceManager::Proxy(TracedTraceManager::new(Channel(
                grant.endpoint,
            )));
            log("debugd: traced proxy connected\n");
        }
        s
    };
    serve(state).await
}

async fn serve(mut state: migration::Runtime) -> ! {
    use bexos_userspace::live_migration::State;
    let mut source = bexos_userspace::live_migration::Source::new(state.migration);
    let mut transport =
        bexos_debugd::transport::SerialByteTransport::new(Channel(state.uart_handle));
    let mut kernel_debug = KernelDebugControlPublicClient::new(KernelTransport(6));
    let mut logged_debug_byte = false;
    log("debugd: serve loop ready\n");
    loop {
        if source.poll(&state).is_err() {
            let _ = bexos_userspace::migration::abort();
        }
        if source.quiescing() {
            state.shells.renew();
            bexos_userspace::yield_now();
            continue;
        }
        if state.shells.expire() {
            source.changed(0);
        }
        let awaiting = state.platform.awaiting_completion;
        state.platform.observe_completion();
        if awaiting {
            source.changed(0);
        }
        if let Ok(message) = state.control.try_recv() {
            if message.bytes == b"bexos.debugd.handoff.v1" {
                if let Some(handle) = message.handles.first() {
                    let app_manager = message.handles.get(1).copied().map(Channel);
                    state.apps = LiveAppManager::Proxy(LifecycleAppManager::new(
                        Channel(*handle),
                        app_manager,
                    ));
                    source.changed(0);
                    log("debugd: app lifecycle proxy connected\n");
                }
                if let Some(handle) = message.handles.get(2) {
                    state.users = LiveUserManager::Proxy(UserServiceManager::new(Channel(*handle)));
                    source.changed(0);
                    log("debugd: user proxy connected\n");
                }
                if let Some(handle) = message.handles.get(3) {
                    state.tee = LiveTeeManager::Proxy(TeeServiceManager::new(Channel(*handle)));
                    source.changed(0);
                    log("debugd: tee proxy connected\n");
                }
                if let Some(handle) = message.handles.get(4) {
                    state.updates =
                        LiveUpdateManager::Proxy(UpdateServiceManager::new(Channel(*handle)));
                    source.changed(0);
                    log("debugd: updated proxy connected\n");
                }
                let _ = state.control.send(b"bexos.debugd.handoff.ok", &[]);
            }
        }
        // A partial frame must not block migration traffic. Bound UART work per
        // turn and preserve the exact bytes already consumed from the transport.
        let old_input_keys: alloc::vec::Vec<_> = bexos_migration::blob::keys(0, &state.input)
            .map(|k| migration::INPUT | k)
            .collect();
        let mut read = false;
        let uart_budget = if source.active() {
            bexos_userspace::syscall::frequency() / 200
        } else {
            bexos_userspace::syscall::frequency() / 10
        };
        let uart_deadline = bexos_userspace::syscall::ticks() + uart_budget;
        for _ in 0..65536 {
            // Read directly into retained input state. No transport-local prefetch
            // buffer may be lost when the service heart transplant commits.
            if matches!(
                parse_frame(&state.input),
                Err(bexos_debug_wire::WireError::Incomplete)
            ) {
                let mut bytes = [0; 4096];
                let count = loop {
                    let count = transport.try_read_bytes(&mut bytes);
                    if count != 0 {
                        break count;
                    }
                    if state.input.is_empty() || bexos_userspace::syscall::ticks() >= uart_deadline
                    {
                        break 0;
                    }
                    core::hint::spin_loop();
                };
                if count == 0 {
                    break;
                }
                state.input.extend_from_slice(&bytes[..count]);
                // A large shell frame can take longer than the terminal lease to
                // arrive under TCG. Receiving transport progress proves that the
                // client is still connected; only idle/disconnected sessions
                // should expire while a frame is incomplete.
                state.shells.renew();
                if !logged_debug_byte {
                    logged_debug_byte = true;
                    log("debugd: first debug byte read\n");
                }
            }
            read = true;
            match parse_frame(&state.input) {
                Ok((frame, used)) => {
                    log("debugd: decoded debug frame\n");
                    state.input.drain(..used);
                    state.shells.renew();
                    let changes = frame.method_id != bexos_debug_wire::METHOD_HEALTH_CHECK
                        && frame.method_id != bexos_debug_wire::METHOD_LIST_APPS
                        && frame.method_id != bexos_debug_wire::METHOD_LIST_PROCESSES
                        && frame.method_id != bexos_debug_wire::METHOD_GET_COMPONENT_CONFIG
                        && (frame.method_id != bexos_debug_wire::METHOD_EXEC_COMMAND
                            || bexos_debug_wire::decode_exec_request(&frame.payload)
                                .is_ok_and(|q| q.component_id.contains("apply_")));
                    let prior = if source.active() && changes {
                        state.keys()
                    } else {
                        alloc::vec::Vec::new()
                    };
                    let response = if (bexos_debug_wire::METHOD_SHELL_OPEN
                        ..=bexos_debug_wire::METHOD_SHELL_CLOSE)
                        .contains(&frame.method_id)
                    {
                        let response = match bexos_debug_wire::decode_shell_request(&frame.payload)
                        {
                            Ok(q) => {
                                state
                                    .shells
                                    .handle(frame.method_id, q, &mut state.apps, &mut state.users)
                                    .await
                            }
                            Err(_) => bexos_debug_wire::ShellResponse {
                                status: -8,
                                message: "invalid shell request".into(),
                                ..Default::default()
                            },
                        };
                        let mut payload = alloc::vec::Vec::new();
                        bexos_debug_wire::encode_shell_response(&response, &mut payload);
                        bexos_debug_wire::Frame {
                            payload,
                            ..frame.clone()
                        }
                    } else if frame.method_id == bexos_debug_wire::METHOD_LIST_PROCESSES {
                        state
                            .traces
                            .record_debug_event("debugd:list_processes")
                            .await;
                        bexos_debugd::processes::response(&frame, &mut kernel_debug)
                    } else if frame.method_id == bexos_debug_wire::METHOD_EXEC_COMMAND
                        && bexos_debug_wire::decode_exec_request(&frame.payload)
                            .is_ok_and(|q| q.component_id == "kernel.ps")
                    {
                        bexos_debugd::processes::exec_response(&frame, &mut kernel_debug)
                    } else {
                        handle_frame_with_trace_backends(
                            &frame,
                            &[],
                            &mut state.installer,
                            &mut state.apps,
                            &mut state.updates,
                            &mut state.platform,
                            &mut state.users,
                            &mut state.tee,
                            &mut state.traces,
                        )
                        .await
                    };
                    let _ = write_frame_async(&mut transport, &response).await;
                    state.shells.renew();
                    state.platform.commit_if_pending();
                    if source.active() && changes {
                        source.changed_keys(prior);
                        source.changed_keys(state.keys());
                    }
                    break;
                }
                Err(bexos_debug_wire::WireError::Incomplete) => continue,
                Err(_) => {
                    state.input.clear();
                    log("debugd: dropped malformed debug frame\n");
                    break;
                }
            }
        }
        if read {
            source.changed_keys(old_input_keys);
            source.changed(0);
            source.changed_keys(
                bexos_migration::blob::keys(0, &state.input).map(|k| migration::INPUT | k),
            );
        }
        bexos_userspace::yield_now();
    }
}

enum LiveAppManager {
    Unsupported(UnsupportedAppManager),
    Proxy(LifecycleAppManager),
}

enum LiveUserManager {
    Unsupported(UnsupportedUserManager),
    Proxy(UserServiceManager),
}

enum LiveTeeManager {
    Unsupported(UnsupportedTeeManager),
    Proxy(TeeServiceManager),
}

enum LiveUpdateManager {
    Unsupported(UnsupportedUpdateManager),
    Proxy(UpdateServiceManager),
}

impl bexos_debugd::service::UpdateManager for LiveUpdateManager {
    async fn apply_firmware_from_feed(
        &mut self,
        selector: u32,
        on_reboot: bool,
    ) -> bexos_debug_wire::DebugStatusResponse {
        match self {
            Self::Unsupported(manager) => {
                manager.apply_firmware_from_feed(selector, on_reboot).await
            }
            Self::Proxy(manager) => manager.apply_firmware_from_feed(selector, on_reboot).await,
        }
    }
    async fn apply_firmware(&mut self, on_reboot: bool) -> bexos_debug_wire::DebugStatusResponse {
        match self {
            Self::Unsupported(manager) => manager.apply_firmware(on_reboot).await,
            Self::Proxy(manager) => manager.apply_firmware(on_reboot).await,
        }
    }
    async fn check_updates(&mut self, request: UpdateCheckRequest) -> UpdateCheckResponse {
        match self {
            Self::Unsupported(manager) => manager.check_updates(request).await,
            Self::Proxy(manager) => manager.check_updates(request).await,
        }
    }

    async fn stage_from_feed(
        &mut self,
        request: UpdateCheckRequest,
    ) -> bexos_debug_wire::DebugStatusResponse {
        match self {
            Self::Unsupported(manager) => manager.stage_from_feed(request).await,
            Self::Proxy(manager) => manager.stage_from_feed(request).await,
        }
    }

    async fn apply_from_feed(
        &mut self,
        request: UpdateCheckRequest,
    ) -> bexos_debug_wire::DebugStatusResponse {
        match self {
            Self::Unsupported(manager) => manager.apply_from_feed(request).await,
            Self::Proxy(manager) => manager.apply_from_feed(request).await,
        }
    }

    async fn apply_service<A: AppManager>(
        &mut self,
        apps: &mut A,
    ) -> bexos_debug_wire::DebugStatusResponse {
        match self {
            Self::Unsupported(manager) => manager.apply_service(apps).await,
            Self::Proxy(manager) => manager.apply_service().await,
        }
    }

    async fn begin_upload(
        &mut self,
        request: bexos_debug_wire::UpdateUploadBeginRequest,
    ) -> bexos_debug_wire::DebugStatusResponse {
        match self {
            Self::Unsupported(manager) => manager.begin_upload(request).await,
            Self::Proxy(manager) => manager.begin_upload(request).await,
        }
    }

    async fn write_chunk(
        &mut self,
        request: bexos_debug_wire::UpdateChunkRequest,
    ) -> bexos_debug_wire::DebugStatusResponse {
        match self {
            Self::Unsupported(manager) => manager.write_chunk(request).await,
            Self::Proxy(manager) => manager.write_chunk(request).await,
        }
    }

    async fn commit_upload(
        &mut self,
        request: bexos_debug_wire::UpdateUploadCommitRequest,
    ) -> bexos_debug_wire::DebugStatusResponse {
        match self {
            Self::Unsupported(manager) => manager.commit_upload(request).await,
            Self::Proxy(manager) => manager.commit_upload(request).await,
        }
    }

    async fn apply_app<A: AppManager>(
        &mut self,
        apps: &mut A,
    ) -> bexos_debug_wire::DebugStatusResponse {
        match self {
            Self::Unsupported(manager) => manager.apply_app(apps).await,
            Self::Proxy(manager) => manager.apply_app().await,
        }
    }

    async fn apply_platform<P: PlatformUpdateApplier, T: TeeManager>(
        &mut self,
        platform: &mut P,
        tee: &mut T,
    ) -> bexos_debug_wire::DebugStatusResponse {
        match self {
            Self::Unsupported(manager) => manager.apply_platform(platform, tee).await,
            Self::Proxy(manager) => {
                let response = manager.apply_platform().await;
                if response.status == 0
                    && response.message.contains("kernel platform update staged")
                {
                    platform.external_update_started();
                }
                response
            }
        }
    }

    async fn status(&mut self) -> bexos_debug_wire::DebugStatusResponse {
        match self {
            Self::Unsupported(manager) => manager.status().await,
            Self::Proxy(manager) => manager.status().await,
        }
    }
}

impl AppManager for LiveAppManager {
    async fn terminate_selected_shell(
        &mut self,
        package: &str,
        uid: u64,
    ) -> bexos_debug_wire::DebugStatusResponse {
        match self {
            Self::Proxy(manager) => manager.terminate_selected_shell(package, uid).await,
            _ => debug_status(-95, "appd unavailable"),
        }
    }

    async fn preferences(
        &mut self,
        request: bexos_debug_wire::PreferencesRequest,
    ) -> bexos_debug_wire::PreferencesResponse {
        match self {
            Self::Unsupported(manager) => manager.preferences(request).await,
            Self::Proxy(manager) => manager.preferences(request).await,
        }
    }
    async fn get_component_config(
        &mut self,
        request: bexos_debug_wire::ComponentConfigGetRequest,
    ) -> bexos_debug_wire::ComponentConfigGetResponse {
        match self {
            Self::Unsupported(manager) => manager.get_component_config(request).await,
            Self::Proxy(manager) => manager.get_component_config(request).await,
        }
    }
    async fn set_component_config(
        &mut self,
        request: bexos_debug_wire::ComponentConfigSetRequest,
    ) -> bexos_debug_wire::ComponentConfigMutationResponse {
        match self {
            Self::Unsupported(manager) => manager.set_component_config(request).await,
            Self::Proxy(manager) => manager.set_component_config(request).await,
        }
    }
    async fn reset_component_config(
        &mut self,
        request: bexos_debug_wire::ComponentConfigResetRequest,
    ) -> bexos_debug_wire::ComponentConfigMutationResponse {
        match self {
            Self::Unsupported(manager) => manager.reset_component_config(request).await,
            Self::Proxy(manager) => manager.reset_component_config(request).await,
        }
    }

    async fn process_progress(&mut self, package: &str) -> bexos_debug_wire::DebugStatusResponse {
        match self {
            Self::Proxy(m) => m.process_progress(package).await,
            _ => debug_status(-95, "appd unavailable"),
        }
    }
    async fn migrate_service(
        &mut self,
        generation: u64,
        target: &str,
        artifact: &[u8],
    ) -> bexos_debug_wire::DebugStatusResponse {
        match self {
            Self::Proxy(m) => m.migrate_service(generation, target, artifact).await,
            _ => debug_status(-95, "appd unavailable"),
        }
    }
    async fn migrate_service_from_store(
        &mut self,
        archive_id: &str,
        generation: u64,
        target: &str,
    ) -> bexos_debug_wire::DebugStatusResponse {
        match self {
            Self::Proxy(m) => {
                m.migrate_service_from_store(archive_id, generation, target)
                    .await
            }
            _ => debug_status(-95, "appd unavailable"),
        }
    }
    async fn migration_status(&mut self, target: &str) -> bexos_debug_wire::DebugStatusResponse {
        match self {
            Self::Proxy(m) => m.migration_status(target).await,
            _ => debug_status(-95, "appd unavailable"),
        }
    }

    async fn list_apps(
        &mut self,
    ) -> Result<alloc::vec::Vec<bexos_debug_wire::AppInfo>, bexos_debug_wire::DebugStatusResponse>
    {
        match self {
            Self::Unsupported(manager) => manager.list_apps().await,
            Self::Proxy(manager) => manager.list_apps().await,
        }
    }

    async fn begin_bundle_upload(
        &mut self,
        request: bexos_debug_wire::AppBundleUploadBeginRequest,
    ) -> bexos_debug_wire::DebugStatusResponse {
        match self {
            Self::Unsupported(manager) => manager.begin_bundle_upload(request).await,
            Self::Proxy(manager) => manager.begin_bundle_upload(request).await,
        }
    }

    async fn write_bundle_chunk(
        &mut self,
        request: bexos_debug_wire::AppBundleChunkRequest,
    ) -> bexos_debug_wire::DebugStatusResponse {
        match self {
            Self::Unsupported(manager) => manager.write_bundle_chunk(request).await,
            Self::Proxy(manager) => manager.write_bundle_chunk(request).await,
        }
    }

    async fn commit_bundle_upload(
        &mut self,
        request: bexos_debug_wire::AppBundleUploadCommitRequest,
    ) -> bexos_debug_wire::DebugStatusResponse {
        match self {
            Self::Unsupported(manager) => manager.commit_bundle_upload(request).await,
            Self::Proxy(manager) => manager.commit_bundle_upload(request).await,
        }
    }

    async fn uninstall(
        &mut self,
        request: bexos_debug_wire::AppUninstallRequest,
    ) -> bexos_debug_wire::DebugStatusResponse {
        match self {
            Self::Unsupported(manager) => manager.uninstall(request).await,
            Self::Proxy(manager) => manager.uninstall(request).await,
        }
    }

    async fn launch(
        &mut self,
        request: bexos_debug_wire::AppLaunchRequest,
    ) -> bexos_debug_wire::DebugStatusResponse {
        match self {
            Self::Unsupported(manager) => manager.launch(request).await,
            Self::Proxy(manager) => manager.launch(request).await,
        }
    }
}

impl UserManager for LiveUserManager {
    async fn list_users(
        &mut self,
    ) -> Result<alloc::vec::Vec<bexos_debug_wire::UserInfo>, bexos_debug_wire::DebugStatusResponse>
    {
        match self {
            Self::Unsupported(manager) => manager.list_users().await,
            Self::Proxy(manager) => manager.list_users().await,
        }
    }

    async fn get_user(
        &mut self,
        request: bexos_debug_wire::UserGetRequest,
    ) -> Result<bexos_debug_wire::UserInfo, bexos_debug_wire::DebugStatusResponse> {
        match self {
            Self::Unsupported(manager) => manager.get_user(request).await,
            Self::Proxy(manager) => manager.get_user(request).await,
        }
    }

    async fn create_user(
        &mut self,
        request: bexos_debug_wire::UserCreateRequest,
    ) -> bexos_debug_wire::DebugStatusResponse {
        match self {
            Self::Unsupported(manager) => manager.create_user(request).await,
            Self::Proxy(manager) => manager.create_user(request).await,
        }
    }

    async fn update_user(
        &mut self,
        request: bexos_debug_wire::UserUpdateRequest,
    ) -> bexos_debug_wire::DebugStatusResponse {
        match self {
            Self::Unsupported(manager) => manager.update_user(request).await,
            Self::Proxy(manager) => manager.update_user(request).await,
        }
    }

    async fn delete_user(
        &mut self,
        request: bexos_debug_wire::UserDeleteRequest,
    ) -> bexos_debug_wire::DebugStatusResponse {
        match self {
            Self::Unsupported(manager) => manager.delete_user(request).await,
            Self::Proxy(manager) => manager.delete_user(request).await,
        }
    }

    async fn unlock_user(
        &mut self,
        request: bexos_debug_wire::UserUnlockRequest,
    ) -> bexos_debug_wire::DebugStatusResponse {
        match self {
            Self::Unsupported(manager) => manager.unlock_user(request).await,
            Self::Proxy(manager) => manager.unlock_user(request).await,
        }
    }

    async fn lock_user(
        &mut self,
        request: bexos_debug_wire::UserLockRequest,
    ) -> bexos_debug_wire::DebugStatusResponse {
        match self {
            Self::Unsupported(manager) => manager.lock_user(request).await,
            Self::Proxy(manager) => manager.lock_user(request).await,
        }
    }
}

impl TeeManager for LiveTeeManager {
    async fn concurrent_storage_probe(&mut self) -> bexos_debug_wire::DebugStatusResponse {
        match self {
            Self::Unsupported(manager) => manager.concurrent_storage_probe().await,
            Self::Proxy(manager) => manager.concurrent_storage_probe().await,
        }
    }
    async fn info(&mut self) -> bexos_debug_wire::TeeInfoResponse {
        match self {
            Self::Unsupported(manager) => manager.info().await,
            Self::Proxy(manager) => manager.info().await,
        }
    }

    async fn list_apps(&mut self) -> bexos_debug_wire::TeeAppListResponse {
        match self {
            Self::Unsupported(manager) => manager.list_apps().await,
            Self::Proxy(manager) => manager.list_apps().await,
        }
    }

    async fn install_app(
        &mut self,
        request: bexos_debug_wire::TeeInstallAppRequest,
    ) -> bexos_debug_wire::TeeInstallAppResponse {
        match self {
            Self::Unsupported(manager) => manager.install_app(request).await,
            Self::Proxy(manager) => manager.install_app(request).await,
        }
    }

    async fn uninstall_app(
        &mut self,
        request: bexos_debug_wire::TeeUuidRequest,
    ) -> bexos_debug_wire::DebugStatusResponse {
        match self {
            Self::Unsupported(manager) => manager.uninstall_app(request).await,
            Self::Proxy(manager) => manager.uninstall_app(request).await,
        }
    }

    async fn open_session(
        &mut self,
        request: bexos_debug_wire::TeeUuidRequest,
    ) -> bexos_debug_wire::TeeOpenSessionResponse {
        match self {
            Self::Unsupported(manager) => manager.open_session(request).await,
            Self::Proxy(manager) => manager.open_session(request).await,
        }
    }

    async fn close_session(
        &mut self,
        request: bexos_debug_wire::TeeSessionRequest,
    ) -> bexos_debug_wire::DebugStatusResponse {
        match self {
            Self::Unsupported(manager) => manager.close_session(request).await,
            Self::Proxy(manager) => manager.close_session(request).await,
        }
    }

    async fn invoke(
        &mut self,
        request: bexos_debug_wire::TeeInvokeRequest,
    ) -> bexos_debug_wire::TeeInvokeResponse {
        match self {
            Self::Unsupported(manager) => manager.invoke(request).await,
            Self::Proxy(manager) => manager.invoke(request).await,
        }
    }

    async fn update_core(
        &mut self,
        request: bexos_debug_wire::TeeUpdateCoreRequest,
    ) -> bexos_debug_wire::TeeUpdateCoreResponse {
        match self {
            Self::Unsupported(manager) => manager.update_core(request).await,
            Self::Proxy(manager) => manager.update_core(request).await,
        }
    }

    async fn update_status(&mut self) -> bexos_debug_wire::TeeUpdateStatusResponse {
        match self {
            Self::Unsupported(manager) => manager.update_status().await,
            Self::Proxy(manager) => manager.update_status().await,
        }
    }
}

fn tee_status_response<F>(
    response: Result<
        (alloc::vec::Vec<u8>, alloc::vec::Vec<tee_fidl::HandleRef>),
        tee_fidl::FidlWireError,
    >,
    decode: F,
) -> bexos_debug_wire::DebugStatusResponse
where
    F: FnOnce(&[u8], &[tee_fidl::HandleRef]) -> Result<i32, tee_fidl::FidlWireError>,
{
    let Ok((bytes, handles)) = response else {
        return debug_status(-6, "tee transport");
    };
    match decode(&bytes, &handles) {
        Ok(0) => debug_status(0, "tee operation complete"),
        Ok(status) => debug_status(status, "tee operation failed"),
        Err(_) => debug_status(-6, "tee response decode"),
    }
}

fn uuid16(bytes: &[u8]) -> Option<[u8; 16]> {
    let mut out = [0; 16];
    if bytes.len() != out.len() {
        return None;
    }
    out.copy_from_slice(bytes);
    Some(out)
}

fn hash32(bytes: &[u8]) -> Option<[u8; 32]> {
    let mut out = [0; 32];
    if bytes.len() != out.len() {
        return None;
    }
    out.copy_from_slice(bytes);
    Some(out)
}

fn read_vmo(handle: u64, len: u64) -> Result<alloc::vec::Vec<u8>, Status> {
    if handle == 0 || len == 0 {
        return Ok(alloc::vec::Vec::new());
    }
    let map_len = page_round(len).ok_or(Status::ErrInvalidArgs)?;
    let va = Memory::map(handle, map_len, 2)?;
    let bytes = unsafe { core::slice::from_raw_parts(va as *const u8, len as usize) }.to_vec();
    Memory::unmap(va, map_len)?;
    Ok(bytes)
}

fn page_round(value: u64) -> Option<u64> {
    value.checked_add(4095).map(|n| n & !4095)
}

fn tee_kind_name(kind: tee_fidl::TeeKind) -> &'static str {
    match kind {
        tee_fidl::TeeKind::ArmTrusty => "ArmTrusty",
        tee_fidl::TeeKind::AmdPspTee => "AmdPspTee",
        tee_fidl::TeeKind::IntelSgxTdx => "IntelSgxTdx",
        tee_fidl::TeeKind::AwsNitro => "AwsNitro",
        tee_fidl::TeeKind::SoftwareEmu => "SoftwareEmu",
    }
}

fn tee_update_status_name(status: tee_fidl::TeeUpdateStatus) -> &'static str {
    match status {
        tee_fidl::TeeUpdateStatus::Idle => "Idle",
        tee_fidl::TeeUpdateStatus::Staged => "Staged",
        tee_fidl::TeeUpdateStatus::Applying => "Applying",
        tee_fidl::TeeUpdateStatus::Completed => "Completed",
        tee_fidl::TeeUpdateStatus::Failed => "Failed",
    }
}

fn debug_user_info(user: user_manager::UserInfo<'_>) -> bexos_debug_wire::UserInfo {
    bexos_debug_wire::UserInfo {
        uid: user.uid,
        name: user.name.into(),
        display_name: user.display_name.into(),
        disabled: user.disabled,
        home_path: user.home_path.into(),
        unlocked: user.unlocked,
    }
}

fn status_response<F>(
    response: Result<
        (
            alloc::vec::Vec<u8>,
            alloc::vec::Vec<user_manager::HandleRef>,
        ),
        user_manager::FidlWireError,
    >,
    decode: F,
) -> bexos_debug_wire::DebugStatusResponse
where
    F: FnOnce(&[u8], &[user_manager::HandleRef]) -> Result<i32, user_manager::FidlWireError>,
{
    let Ok((bytes, handles)) = response else {
        return debug_status(-6, "user transport");
    };
    match decode(&bytes, &handles) {
        Ok(0) => debug_status(0, "user operation complete"),
        Ok(status) => debug_status(status, "user operation failed"),
        Err(_) => debug_status(-6, "user response decode"),
    }
}

fn debug_status(status: i32, message: &str) -> bexos_debug_wire::DebugStatusResponse {
    bexos_debug_wire::DebugStatusResponse {
        status,
        message: message.into(),
    }
}

#[path = "runtime/platform.rs"]
mod runtime_platform;
use runtime_platform::*;

#[path = "runtime/updates.rs"]
mod runtime_updates;
use runtime_updates::*;

#[path = "runtime/apps.rs"]
mod runtime_apps;
use runtime_apps::*;

#[path = "runtime/users.rs"]
mod runtime_users;
use runtime_users::*;

#[path = "runtime/tee.rs"]
mod runtime_tee;
use runtime_tee::*;

#[path = "runtime/app_operations.rs"]
mod runtime_app_operations;
