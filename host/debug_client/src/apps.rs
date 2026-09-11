use super::*;

impl<T: DebugTransport> DebugClient<T> {
    pub fn list_apps(&mut self) -> Result<Vec<AppInfo>, DebugClientError> {
        let mut payload = Vec::new();
        encode_empty(&mut payload);
        let response = self.call(METHOD_LIST_APPS, payload)?;
        let status = decode_debug_status(&response.payload)?;
        check_debug_status(status.status, &status.message)?;
        Ok(decode_app_list(&response.payload)?)
    }

    pub fn install_app_bundle(
        &mut self,
        upload_id: u64,
        archive: &[u8],
    ) -> Result<(), DebugClientError> {
        self.begin_app_bundle_upload(upload_id, archive.len() as u64)?;
        self.write_app_bundle_stream(upload_id, archive)?;
        self.commit_app_bundle_upload(upload_id)
    }

    pub fn uninstall_app(&mut self, package_id: &str) -> Result<(), DebugClientError> {
        let mut payload = Vec::new();
        encode_app_uninstall(
            &AppUninstallRequest {
                package_id: package_id.to_string(),
            },
            &mut payload,
        );
        self.status_call(METHOD_UNINSTALL_APP, payload)
    }

    pub fn launch_app(
        &mut self,
        package_id: &str,
        process_name: &str,
        arg0: u64,
        uid: u64,
    ) -> Result<(), DebugClientError> {
        let mut payload = Vec::new();
        encode_app_launch(
            &AppLaunchRequest {
                package_id: package_id.to_string(),
                process_name: process_name.to_string(),
                arg0,
                uid,
            },
            &mut payload,
        );
        self.status_call(METHOD_LAUNCH_APP, payload)
    }

    pub fn install_app_from_url(&mut self, url: &str) -> Result<(), DebugClientError> {
        let mut payload = Vec::new();
        encode_app_install_from_url(
            &AppInstallFromUrlRequest {
                url: url.to_string(),
            },
            &mut payload,
        );
        self.status_call(METHOD_INSTALL_APP_FROM_URL, payload)
    }

    pub fn reload_well_known(&mut self, domain: &str) -> Result<(), DebugClientError> {
        let mut payload = Vec::new();
        encode_well_known_reload(
            &WellKnownReloadRequest {
                domain: domain.to_string(),
            },
            &mut payload,
        );
        self.status_call(METHOD_RELOAD_WELL_KNOWN, payload)
    }
}
