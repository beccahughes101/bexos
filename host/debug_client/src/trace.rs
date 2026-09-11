use super::*;

impl<T: DebugTransport> DebugClient<T> {
    pub fn trace_start(
        &mut self,
        categories: u32,
        buffer_mode: u32,
        buffer_size_kb: u32,
    ) -> Result<(), DebugClientError> {
        self.trace_start_with_format(categories, buffer_mode, buffer_size_kb, 1)
    }

    pub fn trace_start_with_format(
        &mut self,
        categories: u32,
        buffer_mode: u32,
        buffer_size_kb: u32,
        output_format: u32,
    ) -> Result<(), DebugClientError> {
        let mut payload = Vec::new();
        encode_trace_start_request(
            &TraceStartRequest {
                categories,
                buffer_mode,
                buffer_size_kb,
                output_format,
            },
            &mut payload,
        );
        self.status_call(METHOD_TRACE_START, payload)
    }

    pub fn trace_status(&mut self) -> Result<TraceStatusResponse, DebugClientError> {
        let mut payload = Vec::new();
        encode_empty(&mut payload);
        let response = self.call(METHOD_TRACE_STATUS, payload)?;
        let response = decode_trace_status_response(&response.payload)?;
        check_debug_status(response.status, "trace status failed")?;
        Ok(response)
    }

    pub fn trace_stop(&mut self) -> Result<Vec<u8>, DebugClientError> {
        let mut trace = Vec::new();
        let mut offset = 0;
        loop {
            let mut payload = Vec::new();
            encode_trace_stop_request(
                &TraceStopRequest {
                    offset,
                    max_bytes: 48 * 1024,
                },
                &mut payload,
            );
            let response = self.call(METHOD_TRACE_STOP, payload)?;
            let response = decode_trace_stop_response(&response.payload)?;
            check_debug_status(response.status, "trace stop failed")?;
            if response.offset != offset {
                return Err(DebugClientError::Wire(WireError::InvalidProto));
            }
            trace.extend_from_slice(&response.bytes);
            offset = offset.saturating_add(response.bytes.len() as u64);
            if response.complete {
                if offset != response.total_len {
                    return Err(DebugClientError::Wire(WireError::InvalidProto));
                }
                return Ok(trace);
            }
        }
    }
}
