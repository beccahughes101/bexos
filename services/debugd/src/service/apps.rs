use super::*;

pub trait AppManager {
    async fn terminate_selected_shell(&mut self, _package: &str, _uid: u64) -> DebugStatusResponse {
        unsupported_app_response()
    }
    async fn preferences(
        &mut self,
        _request: bexos_debug_wire::PreferencesRequest,
    ) -> bexos_debug_wire::PreferencesResponse {
        bexos_debug_wire::PreferencesResponse {
            status: -8,
            message: "preferences unavailable".into(),
            ..Default::default()
        }
    }
    async fn process_progress(&mut self, _package: &str) -> DebugStatusResponse {
        unsupported_app_response()
    }
    async fn migrate_service(
        &mut self,
        _generation: u64,
        _target: &str,
        _artifact: &[u8],
    ) -> DebugStatusResponse {
        unsupported_update_response()
    }
    async fn migrate_service_from_store(
        &mut self,
        _archive_id: &str,
        _generation: u64,
        _target: &str,
    ) -> DebugStatusResponse {
        unsupported_update_response()
    }
    async fn migration_status(&mut self, _target: &str) -> DebugStatusResponse {
        unsupported_update_response()
    }

    async fn list_apps(&mut self) -> Result<Vec<AppInfo>, DebugStatusResponse>;
    async fn begin_bundle_upload(
        &mut self,
        request: AppBundleUploadBeginRequest,
    ) -> DebugStatusResponse;
    async fn write_bundle_chunk(&mut self, request: AppBundleChunkRequest) -> DebugStatusResponse;
    async fn commit_bundle_upload(
        &mut self,
        request: AppBundleUploadCommitRequest,
    ) -> DebugStatusResponse;
    async fn uninstall(&mut self, request: AppUninstallRequest) -> DebugStatusResponse;
    async fn launch(&mut self, request: AppLaunchRequest) -> DebugStatusResponse;
    async fn get_component_config(
        &mut self,
        _request: ComponentConfigGetRequest,
    ) -> ComponentConfigGetResponse {
        ComponentConfigGetResponse {
            status: unsupported_app_response().status,
            generation: 0,
            config: Vec::new(),
        }
    }
    async fn set_component_config(
        &mut self,
        _request: ComponentConfigSetRequest,
    ) -> ComponentConfigMutationResponse {
        ComponentConfigMutationResponse {
            status: unsupported_app_response().status,
            generation: 0,
            message: unsupported_app_response().message,
        }
    }
    async fn reset_component_config(
        &mut self,
        _request: ComponentConfigResetRequest,
    ) -> ComponentConfigMutationResponse {
        ComponentConfigMutationResponse {
            status: unsupported_app_response().status,
            generation: 0,
            message: unsupported_app_response().message,
        }
    }
    async fn install_from_url(
        &mut self,
        _request: AppInstallFromUrlRequest,
    ) -> DebugStatusResponse {
        unsupported_app_response()
    }
    async fn reload_well_known(&mut self, _request: WellKnownReloadRequest) -> DebugStatusResponse {
        unsupported_app_response()
    }
}

pub struct UnsupportedAppManager;

impl AppManager for UnsupportedAppManager {
    async fn list_apps(&mut self) -> Result<Vec<AppInfo>, DebugStatusResponse> {
        Err(unsupported_app_response())
    }

    async fn begin_bundle_upload(
        &mut self,
        _request: AppBundleUploadBeginRequest,
    ) -> DebugStatusResponse {
        unsupported_app_response()
    }

    async fn write_bundle_chunk(&mut self, _request: AppBundleChunkRequest) -> DebugStatusResponse {
        unsupported_app_response()
    }

    async fn commit_bundle_upload(
        &mut self,
        _request: AppBundleUploadCommitRequest,
    ) -> DebugStatusResponse {
        unsupported_app_response()
    }

    async fn uninstall(&mut self, _request: AppUninstallRequest) -> DebugStatusResponse {
        unsupported_app_response()
    }

    async fn launch(&mut self, _request: AppLaunchRequest) -> DebugStatusResponse {
        unsupported_app_response()
    }
}
