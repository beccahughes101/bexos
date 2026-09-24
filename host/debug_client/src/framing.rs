use super::*;

impl<T: DebugTransport> DebugClient<T> {
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            next_request_id: 1,
            read_buffer: Vec::new(),
            pending_frames: Vec::new(),
            received_trace: Vec::new(),
        }
    }

    /// Bounded incoming wire/serial trace for diagnosing guest transitions.
    pub fn received_trace(&self) -> &[u8] {
        &self.received_trace
    }

    fn capture_transport_diagnostics(&mut self) {
        let bytes = self.transport.read_diagnostics();
        const LIMIT: usize = 256 * 1024;
        let incoming = &bytes[bytes.len().saturating_sub(LIMIT)..];
        live_trace(incoming);
        let discard = (self.received_trace.len() + incoming.len()).saturating_sub(LIMIT);
        self.received_trace.drain(..discard);
        self.received_trace.extend_from_slice(incoming);
    }

    /// Discards diagnostic bytes already inspected by a long-running test.
    pub fn clear_received_trace(&mut self) {
        self.received_trace.clear();
    }

    pub fn drain_for(&mut self, duration: Duration) -> Result<(), DebugClientError> {
        let deadline = Instant::now().checked_add(duration).ok_or_else(|| {
            std::io::Error::new(ErrorKind::InvalidInput, "drain duration too large")
        })?;
        while Instant::now() < deadline {
            self.parse_incoming()?;
            self.receive_chunk(deadline)?;
        }
        self.parse_incoming()
    }

    fn parse_incoming(&mut self) -> Result<(), DebugClientError> {
        loop {
            match self
                .read_buffer
                .windows(4)
                .position(|b| b == bexos_debug_wire::FRAME_MAGIC)
            {
                Some(n) if n > 0 => {
                    self.read_buffer.drain(..n);
                }
                None => {
                    let keep = self.read_buffer.len().min(3);
                    self.read_buffer.drain(..self.read_buffer.len() - keep);
                    return Ok(());
                }
                _ => {}
            }
            match parse_frame(&self.read_buffer) {
                Ok((frame, used)) => {
                    self.read_buffer.drain(..used);
                    if self.pending_frames.len() >= 256 {
                        return Err(std::io::Error::new(
                            ErrorKind::InvalidData,
                            "too many unsolicited debug responses",
                        )
                        .into());
                    }
                    self.pending_frames.push(frame);
                }
                Err(WireError::Incomplete) => return Ok(()),
                Err(_) if !self.read_buffer.is_empty() => {
                    self.read_buffer.drain(..1);
                }
                Err(_) => return Ok(()),
            }
        }
    }
    fn receive_chunk(&mut self, deadline: Instant) -> Result<(), DebugClientError> {
        self.transport.set_read_timeout(
            deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(100))
                .max(Duration::from_millis(1)),
        )?;
        self.capture_transport_diagnostics();
        let mut chunk = [0; 4096];
        let n = match self.transport.read_chunk(&mut chunk) {
            Ok(0) => {
                return Err(std::io::Error::new(
                    ErrorKind::UnexpectedEof,
                    "debug connection closed before a complete response",
                )
                .into());
            }
            Ok(n) => n,
            Err(e)
                if matches!(
                    e.kind(),
                    ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
                ) =>
            {
                std::thread::sleep(Duration::from_millis(1));
                return Ok(());
            }
            Err(e) => return Err(e.into()),
        };
        if self.read_buffer.len() + n
            > bexos_debug_wire::MAX_PAYLOAD_LEN + bexos_debug_wire::HEADER_LEN + 4096
        {
            return Err(WireError::PayloadTooLarge.into());
        }
        live_trace(&chunk[..n]);
        self.read_buffer.extend_from_slice(&chunk[..n]);
        const TRACE_LIMIT: usize = 256 * 1024;
        if self.received_trace.len() + n > TRACE_LIMIT {
            self.received_trace
                .drain(..self.received_trace.len() + n - TRACE_LIMIT);
        }
        self.received_trace.extend_from_slice(&chunk[..n]);
        Ok(())
    }

    pub(super) fn read_update_acks(
        &mut self,
        _stream: u32,
        requests: &mut Vec<(u32, usize)>,
    ) -> Result<(), DebugClientError> {
        for (request_id, _index) in requests.drain(..) {
            let response = self.read_response(
                request_id,
                METHOD_WRITE_UPDATE_CHUNK,
                // A maximum-size update frame can take more than the ordinary
                // 120-second call budget to cross an ARM UART under TCG. Keep
                // the bound finite, but match other on-device update and user
                // operations so slow emulation does not abandon a live upload.
                self.response_deadline_with_default(300),
            )?;
            let status = decode_debug_status(&response.payload)?;
            if status.status != 0 {
                return Err(DebugClientError::RemoteStatus(status));
            }
        }
        Ok(())
    }

    pub(super) fn status_call(
        &mut self,
        method_id: u32,
        payload: Vec<u8>,
    ) -> Result<(), DebugClientError> {
        let response = self.call(method_id, payload)?;
        let status = decode_debug_status(&response.payload)?;
        if status.status == 0 {
            Ok(())
        } else {
            Err(DebugClientError::RemoteStatus(status))
        }
    }

    pub(super) fn call(
        &mut self,
        method_id: u32,
        payload: Vec<u8>,
    ) -> Result<Frame, DebugClientError> {
        // A cold component launch can include validated on-device compilation
        // after package I/O. Its debugd-to-appd envelope allows 900 seconds.
        let timeout = match method_id {
            METHOD_LAUNCH_APP => 600,
            bexos_debug_wire::METHOD_SHELL_OPEN
            | METHOD_COMMIT_APP_BUNDLE_UPLOAD
            | bexos_debug_wire::METHOD_CREATE_USER
            | bexos_debug_wire::METHOD_UPDATE_USER
            | bexos_debug_wire::METHOD_DELETE_USER
            | bexos_debug_wire::METHOD_UNLOCK_USER
            | bexos_debug_wire::METHOD_LOCK_USER => 300,
            _ => 60,
        };
        let deadline = self.response_deadline_with_default(timeout);
        let request_id = self.send_frame(method_id, payload)?;
        self.read_response(request_id, method_id, deadline)
    }

    pub(super) fn call_with_timeout(
        &mut self,
        method_id: u32,
        payload: Vec<u8>,
        timeout: u64,
    ) -> Result<Frame, DebugClientError> {
        let deadline = self.response_deadline_with_default(timeout);
        let request_id = self.send_frame(method_id, payload)?;
        self.read_response(request_id, method_id, deadline)
    }

    /// Queue an ordered group before waiting for any response. This avoids a
    /// host round trip between related requests; responses retain input order.
    pub fn call_batch(
        &mut self,
        requests: &[(u32, Vec<u8>)],
    ) -> Result<Vec<Frame>, DebugClientError> {
        if requests.len() > 64 || self.next_request_id == 0 {
            return Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                "debug batch exceeds 64 requests or request IDs are exhausted",
            )
            .into());
        }
        let deadline = self.response_deadline();
        let mut bytes = Vec::new();
        let mut pending = Vec::new();
        for (method_id, payload) in requests {
            let request_id = self.next_request_id;
            self.next_request_id = self.next_request_id.checked_add(1).ok_or_else(|| {
                std::io::Error::new(ErrorKind::InvalidInput, "debug request IDs exhausted")
            })?;
            Frame {
                flags: 0,
                request_id,
                method_id: *method_id,
                payload: payload.clone(),
            }
            .encode(&mut bytes)?;
            pending.push((request_id, *method_id));
        }
        self.transport.write_all(&bytes)?;
        pending
            .into_iter()
            .map(|(id, method)| self.read_response(id, method, deadline))
            .collect()
    }

    pub(super) fn send_frame(
        &mut self,
        method_id: u32,
        payload: Vec<u8>,
    ) -> Result<u32, DebugClientError> {
        let request_id = self.next_request_id;
        if request_id == 0 {
            return Err(std::io::Error::new(
                ErrorKind::Other,
                "debug request IDs exhausted; reconnect",
            )
            .into());
        }
        self.next_request_id = self.next_request_id.checked_add(1).unwrap_or(0);
        let frame = Frame {
            flags: 0,
            request_id,
            method_id,
            payload,
        };
        let mut bytes = Vec::new();
        frame.encode(&mut bytes)?;
        self.transport.write_all(&bytes)?;
        Ok(request_id)
    }

    pub(super) fn response_deadline(&self) -> Instant {
        self.response_deadline_with_default(60)
    }

    fn response_deadline_with_default(&self, default_seconds: u64) -> Instant {
        let timeout = std::env::var("BEXOS_DEBUG_CALL_TIMEOUT_SECONDS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .map(Duration::from_secs)
            .filter(|d| !d.is_zero() && *d <= Duration::from_secs(86400))
            .unwrap_or(Duration::from_secs(default_seconds));
        Instant::now() + timeout
    }

    pub(super) fn read_response(
        &mut self,
        request_id: u32,
        method_id: u32,
        deadline: Instant,
    ) -> Result<Frame, DebugClientError> {
        loop {
            self.parse_incoming()?;
            if let Some(i) = self
                .pending_frames
                .iter()
                .position(|f| f.request_id == request_id)
            {
                let frame = self.pending_frames.remove(i);
                if frame.method_id != method_id {
                    return Err(DebugClientError::MethodMismatch {
                        expected: method_id,
                        actual: frame.method_id,
                    });
                }
                return Ok(frame);
            }
            if Instant::now() >= deadline {
                return Err(std::io::Error::new(
                    ErrorKind::TimedOut,
                    "debug response deadline expired",
                )
                .into());
            }
            self.receive_chunk(deadline)?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bexos_debug_wire::encode_health_response;
    struct Transport {
        bytes: Vec<u8>,
        timeouts: usize,
        writes: usize,
    }
    impl DebugTransport for Transport {
        fn write_all(&mut self, _: &[u8]) -> std::io::Result<()> {
            self.writes += 1;
            Ok(())
        }
        fn read_chunk(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            if self.timeouts > 0 {
                self.timeouts -= 1;
                return Err(std::io::Error::from(if self.timeouts % 2 == 0 {
                    ErrorKind::TimedOut
                } else {
                    ErrorKind::WouldBlock
                }));
            }
            let n = out.len().min(self.bytes.len()).min(3);
            out[..n].copy_from_slice(&self.bytes[..n]);
            self.bytes.drain(..n);
            Ok(n)
        }
    }
    #[test]
    fn request_id_exhaustion_never_reuses_an_id() {
        let mut c = DebugClient::new(Transport {
            bytes: Vec::new(),
            timeouts: 0,
            writes: 0,
        });
        c.next_request_id = u32::MAX;
        assert_eq!(c.send_frame(1, Vec::new()).unwrap(), u32::MAX);
        assert!(c.send_frame(1, Vec::new()).is_err());
        assert_eq!(c.transport.writes, 1);
    }
    #[test]
    fn retries_both_timeout_variants_and_partial_frames() {
        let mut payload = Vec::new();
        encode_health_response(
            &HealthCheckResponse {
                service_name: "debugd".into(),
                status: "SERVING".into(),
                version: "v1".into(),
            },
            &mut payload,
        );
        let mut bytes = Vec::new();
        Frame {
            flags: 0,
            request_id: 1,
            method_id: METHOD_HEALTH_CHECK,
            payload,
        }
        .encode(&mut bytes)
        .unwrap();
        let mut c = DebugClient::new(Transport {
            bytes,
            timeouts: 2,
            writes: 0,
        });
        assert_eq!(c.health_check().unwrap().status, "SERVING");
    }
    #[test]
    fn serial_noise_and_unsolicited_frames_are_bounded() {
        let mut c = DebugClient::new(Transport {
            bytes: Vec::new(),
            timeouts: 0,
            writes: 0,
        });
        for _ in 0..1000 {
            c.read_buffer.extend_from_slice(&[b'x'; 4096]);
            c.parse_incoming().unwrap();
            assert!(c.read_buffer.len() <= 3);
        }
        let mut bytes = Vec::new();
        Frame {
            flags: 0,
            request_id: 2,
            method_id: 1,
            payload: Vec::new(),
        }
        .encode(&mut bytes)
        .unwrap();
        for _ in 0..256 {
            c.read_buffer.extend(&bytes);
            c.parse_incoming().unwrap();
        }
        c.read_buffer.extend(bytes);
        assert!(c.parse_incoming().is_err());
    }
}

// Opt-in incoming diagnostics for long QEMU acceptance runs. Requests (including
// passwords) are never sent to this sink. Normal CLI output is unchanged.
fn live_trace(bytes: &[u8]) {
    if !bytes.is_empty()
        && std::env::var_os("BEXOS_DEBUG_LIVE_TRACE").as_deref() == Some(std::ffi::OsStr::new("1"))
    {
        use std::io::Write;
        let _ = std::io::stderr().lock().write_all(bytes);
    }
}
