use super::*;

impl<T: DebugTransport> DebugClient<T> {
    pub fn tee_info(&mut self) -> Result<TeeInfoResponse, DebugClientError> {
        let mut payload = Vec::new();
        encode_empty(&mut payload);
        let response = self.call(METHOD_TEE_INFO, payload)?;
        let response = decode_tee_info_response(&response.payload)?;
        check_debug_status(response.status, "tee info failed")?;
        Ok(response)
    }

    pub fn tee_apps(&mut self) -> Result<Vec<TeeAppInfo>, DebugClientError> {
        let mut payload = Vec::new();
        encode_empty(&mut payload);
        let response = self.call(METHOD_TEE_LIST_APPS, payload)?;
        let response = decode_tee_app_list_response(&response.payload)?;
        check_debug_status(response.status, "tee app list failed")?;
        Ok(response.apps)
    }

    pub fn tee_install_app(&mut self, payload_bytes: Vec<u8>) -> Result<Vec<u8>, DebugClientError> {
        let mut payload = Vec::new();
        encode_tee_install_app_request(
            &TeeInstallAppRequest {
                payload: payload_bytes,
            },
            &mut payload,
        );
        let response = self.call(METHOD_TEE_INSTALL_APP, payload)?;
        let response: TeeInstallAppResponse = decode_tee_install_app_response(&response.payload)?;
        check_debug_status(response.status, "tee app install failed")?;
        Ok(response.uuid)
    }

    pub fn tee_uninstall_app(&mut self, uuid: Vec<u8>) -> Result<(), DebugClientError> {
        let mut payload = Vec::new();
        encode_tee_uuid_request(&TeeUuidRequest { uuid }, &mut payload);
        self.status_call(METHOD_TEE_UNINSTALL_APP, payload)
    }

    pub fn tee_open_session(&mut self, uuid: Vec<u8>) -> Result<u64, DebugClientError> {
        let mut payload = Vec::new();
        encode_tee_uuid_request(&TeeUuidRequest { uuid }, &mut payload);
        let response = self.call(METHOD_TEE_OPEN_SESSION, payload)?;
        let response: TeeOpenSessionResponse = decode_tee_open_session_response(&response.payload)?;
        check_debug_status(response.status, "tee open session failed")?;
        Ok(response.session_id)
    }

    pub fn tee_close_session(&mut self, session_id: u64) -> Result<(), DebugClientError> {
        let mut payload = Vec::new();
        encode_tee_session_request(&TeeSessionRequest { session_id }, &mut payload);
        self.status_call(METHOD_TEE_CLOSE_SESSION, payload)
    }

    pub fn tee_invoke(
        &mut self,
        session_id: u64,
        command_id: u32,
        request: Vec<u8>,
    ) -> Result<Vec<u8>, DebugClientError> {
        let mut payload = Vec::new();
        encode_tee_invoke_request(
            &TeeInvokeRequest {
                session_id,
                command_id,
                payload: request,
            },
            &mut payload,
        );
        let response = self.call(METHOD_TEE_INVOKE, payload)?;
        let response: TeeInvokeResponse = decode_tee_invoke_response(&response.payload)?;
        check_debug_status(response.status, "tee invoke failed")?;
        Ok(response.response)
    }

    pub fn tee_update_core(
        &mut self,
        generation: u64,
        target: String,
        artifact_hash: Vec<u8>,
        image: Vec<u8>,
    ) -> Result<TeeUpdateCoreResponse, DebugClientError> {
        let mut payload = Vec::new();
        encode_tee_update_core_request(
            &TeeUpdateCoreRequest {
                generation,
                target,
                artifact_hash,
                image,
            },
            &mut payload,
        );
        let response = self.call(METHOD_TEE_UPDATE_CORE, payload)?;
        let response = decode_tee_update_core_response(&response.payload)?;
        check_debug_status(response.status, "tee core update failed")?;
        Ok(response)
    }

    pub fn tee_update_status(&mut self) -> Result<TeeUpdateStatusResponse, DebugClientError> {
        let mut payload = Vec::new();
        encode_empty(&mut payload);
        let response = self.call(METHOD_TEE_UPDATE_STATUS, payload)?;
        let response = decode_tee_update_status_response(&response.payload)?;
        check_debug_status(response.status, "tee update status failed")?;
        Ok(response)
    }
}
