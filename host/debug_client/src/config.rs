use super::*;

impl<T: DebugTransport> DebugClient<T> {
    pub fn get_component_config(
        &mut self,
        package_id: &str,
    ) -> Result<ComponentConfigGetResponse, DebugClientError> {
        let mut payload = Vec::new();
        encode_component_config_get(
            &ComponentConfigGetRequest {
                package_id: package_id.to_string(),
            },
            &mut payload,
        );
        let response = self.call_with_timeout(METHOD_GET_COMPONENT_CONFIG, payload, 1200)?;
        let response = decode_component_config_get_response(&response.payload)?;
        check_debug_status(response.status, "component config get")?;
        Ok(response)
    }

    pub fn set_component_config(
        &mut self,
        package_id: &str,
        expected_generation: u64,
        config: &[u8],
    ) -> Result<ComponentConfigMutationResponse, DebugClientError> {
        let mut payload = Vec::new();
        encode_component_config_set(
            &ComponentConfigSetRequest {
                package_id: package_id.to_string(),
                expected_generation,
                config: config.to_vec(),
            },
            &mut payload,
        );
        let response = self.call_with_timeout(METHOD_SET_COMPONENT_CONFIG, payload, 1200)?;
        let response = decode_component_config_mutation_response(&response.payload)?;
        check_debug_status(response.status, &response.message)?;
        Ok(response)
    }

    pub fn reset_component_config(
        &mut self,
        package_id: &str,
        expected_generation: u64,
    ) -> Result<ComponentConfigMutationResponse, DebugClientError> {
        let mut payload = Vec::new();
        encode_component_config_reset(
            &ComponentConfigResetRequest {
                package_id: package_id.to_string(),
                expected_generation,
            },
            &mut payload,
        );
        let response = self.call_with_timeout(METHOD_RESET_COMPONENT_CONFIG, payload, 1200)?;
        let response = decode_component_config_mutation_response(&response.payload)?;
        check_debug_status(response.status, &response.message)?;
        Ok(response)
    }
}

impl<T: DebugTransport> DebugClient<T> {
    pub fn preferences(
        &mut self,
        request: &bexos_debug_wire::PreferencesRequest,
    ) -> Result<bexos_debug_wire::PreferencesResponse, DebugClientError> {
        let mut payload = Vec::new();
        bexos_debug_wire::encode_preferences_request(request, &mut payload);
        // Resolution can wait behind appd's bounded storage and launch work.
        // Keep the host deadline longer than the service's reply deadline.
        let r = self.call_with_timeout(bexos_debug_wire::METHOD_PREFERENCES, payload, 1200)?;
        let r = bexos_debug_wire::decode_preferences_response(&r.payload)?;
        if r.status != 1 {
            check_debug_status(
                r.status,
                &format!("{} (generation {})", r.message, r.generation),
            )?;
        }
        Ok(r)
    }
}
