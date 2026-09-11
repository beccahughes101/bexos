use super::*;

impl<T: DebugTransport> DebugClient<T> {
    pub fn health_check(&mut self) -> Result<HealthCheckResponse, DebugClientError> {
        let mut payload = Vec::new();
        encode_empty(&mut payload);
        let response = self.call(METHOD_HEALTH_CHECK, payload)?;
        Ok(decode_health_response(&response.payload)?)
    }

    /// Inspect kernel process identities and optional resource-group metadata.
    pub fn list_processes(&mut self) -> Result<Vec<ProcessInfo>, DebugClientError> {
        let mut payload = Vec::new();
        encode_empty(&mut payload);
        let response = self.call(METHOD_LIST_PROCESSES, payload)?;
        let status = decode_debug_status(&response.payload)?;
        if status.status != 0 {
            return Err(DebugClientError::RemoteStatus(status));
        }
        Ok(decode_process_list(&response.payload)?)
    }

    pub fn exec_command(
        &mut self,
        component_id: &str,
        args: &[String],
    ) -> Result<ExecResponse, DebugClientError> {
        let mut payload = Vec::new();
        encode_exec_request(
            &ExecRequest {
                component_id: component_id.to_string(),
                args: args.to_vec(),
            },
            &mut payload,
        );
        let response = if matches!(
            component_id,
            "update.apply_app"
                | "update.apply_from_feed"
                | "update.stage_from_feed"
                | "update.apply_service"
                | "update.apply_stored_service"
        ) {
            self.call_with_timeout(METHOD_EXEC_COMMAND, payload, 900)?
        } else {
            self.call(METHOD_EXEC_COMMAND, payload)?
        };
        Ok(decode_exec_response(&response.payload)?)
    }
}
