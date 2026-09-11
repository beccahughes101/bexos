use super::*;

impl<T: DebugTransport> DebugClient<T> {
    pub fn check_updates(
        &mut self,
        selector_kind: u32,
        target: &str,
        all: bool,
        stage: bool,
        apply: bool,
    ) -> Result<UpdateCheckResponse, DebugClientError> {
        let request = UpdateCheckRequest {
            selector_kind,
            target: target.to_string(),
            all,
            stage,
            apply,
        };
        let mut payload = Vec::new();
        encode_update_check_request(&request, &mut payload);
        let method = if apply {
            METHOD_APPLY_UPDATE_FROM_FEED
        } else if stage {
            METHOD_STAGE_UPDATE_FROM_FEED
        } else {
            METHOD_CHECK_UPDATES
        };
        let response = self.call(method, payload)?;
        if apply || stage {
            let status = decode_debug_status(&response.payload)?;
            check_debug_status(status.status, &status.message)?;
            Ok(UpdateCheckResponse {
                status: status.status,
                message: status.message,
                candidates: Vec::new(),
            })
        } else {
            let response = decode_update_check_response(&response.payload)?;
            check_debug_status(response.status, &response.message)?;
            Ok(response)
        }
    }
}
