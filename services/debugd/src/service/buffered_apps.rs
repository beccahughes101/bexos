use super::*;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BufferedAppManager {
    pub(super) upload: Option<BufferedBundleUpload>,
    pub(super) installed: Vec<InstalledBundle>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct BufferedBundleUpload {
    pub(super) upload_id: u64,
    pub(super) archive_len: usize,
    pub(super) archive: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct InstalledBundle {
    pub(super) package_id: String,
    pub(super) name: String,
    pub(super) archive: Vec<u8>,
}

impl BufferedAppManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn seed_app(&mut self, package_id: &str, name: &str) {
        self.installed.push(InstalledBundle {
            package_id: package_id.into(),
            name: name.into(),
            archive: Vec::new(),
        });
    }
}

impl AppManager for BufferedAppManager {
    async fn list_apps(&mut self) -> Result<Vec<AppInfo>, DebugStatusResponse> {
        Ok(self
            .installed
            .iter()
            .map(|app| AppInfo {
                package_id: app.package_id.clone(),
                name: app.name.clone(),
                state: "INSTALLED".into(),
                source: "Debugd".into(),
                protected: false,
            })
            .collect())
    }

    async fn begin_bundle_upload(
        &mut self,
        request: AppBundleUploadBeginRequest,
    ) -> DebugStatusResponse {
        if request.archive_len == 0 || request.archive_len > 32 * 1024 * 1024 {
            return invalid_request_response("invalid app bundle upload dimensions");
        }
        self.upload = Some(BufferedBundleUpload {
            upload_id: request.upload_id,
            archive_len: request.archive_len as usize,
            archive: Vec::new(),
        });
        ok_response("app bundle upload started")
    }

    async fn write_bundle_chunk(&mut self, request: AppBundleChunkRequest) -> DebugStatusResponse {
        let Some(upload) = self.upload.as_mut() else {
            return invalid_request_response("no active app bundle upload");
        };
        if upload.upload_id != request.upload_id {
            return invalid_request_response("app bundle upload id mismatch");
        }
        if request.offset as usize != upload.archive.len() {
            return invalid_request_response("out-of-order app bundle chunk");
        }
        if upload.archive.len().saturating_add(request.bytes.len()) > upload.archive_len {
            return invalid_request_response("app bundle chunk exceeds declared length");
        }
        upload.archive.extend_from_slice(&request.bytes);
        ok_response("app bundle chunk accepted")
    }

    async fn commit_bundle_upload(
        &mut self,
        request: AppBundleUploadCommitRequest,
    ) -> DebugStatusResponse {
        let Some(upload) = self.upload.take() else {
            return invalid_request_response("no active app bundle upload");
        };
        if upload.upload_id != request.upload_id {
            self.upload = Some(upload);
            return invalid_request_response("app bundle upload id mismatch");
        }
        if upload.archive.len() != upload.archive_len {
            self.upload = Some(upload);
            return invalid_request_response("app bundle upload incomplete");
        }
        let package_id = alloc::format!("debug.bundle.{}", self.installed.len() + 1);
        self.installed.push(InstalledBundle {
            name: package_id.clone(),
            package_id,
            archive: upload.archive,
        });
        ok_response("app bundle installed")
    }

    async fn uninstall(&mut self, request: AppUninstallRequest) -> DebugStatusResponse {
        let Some(index) = self
            .installed
            .iter()
            .position(|app| app.package_id == request.package_id)
        else {
            return invalid_request_response("unknown app package");
        };
        self.installed.remove(index);
        ok_response("app uninstalled")
    }

    async fn launch(&mut self, request: AppLaunchRequest) -> DebugStatusResponse {
        if request.package_id.is_empty() || request.process_name.is_empty() {
            return invalid_request_response("launch requires package and process");
        }
        if self
            .installed
            .iter()
            .any(|app| app.package_id == request.package_id)
        {
            ok_response("app launch requested")
        } else {
            invalid_request_response("unknown app package")
        }
    }

    async fn install_from_url(&mut self, request: AppInstallFromUrlRequest) -> DebugStatusResponse {
        if request.url.starts_with("https://") {
            ok_response("app URL install requested")
        } else {
            invalid_request_response("install-url requires https URL")
        }
    }

    async fn reload_well_known(&mut self, request: WellKnownReloadRequest) -> DebugStatusResponse {
        if request.domain.is_empty() {
            invalid_request_response("reload-well-known requires domain")
        } else {
            ok_response("well-known reload requested")
        }
    }
}
