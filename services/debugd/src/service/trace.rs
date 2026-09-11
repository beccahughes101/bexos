use super::*;

pub trait TraceManager {
    async fn start_trace(&mut self, request: TraceStartRequest) -> DebugStatusResponse;
    async fn trace_status(&mut self) -> TraceStatusResponse;
    async fn stop_trace(&mut self, request: TraceStopRequest) -> TraceStopResponse;
    async fn record_debug_event(&mut self, name: &'static str);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UnsupportedTraceManager;

impl TraceManager for UnsupportedTraceManager {
    async fn start_trace(&mut self, _request: TraceStartRequest) -> DebugStatusResponse {
        debug_status(-95, "trace backend unavailable")
    }

    async fn trace_status(&mut self) -> TraceStatusResponse {
        TraceStatusResponse {
            status: -95,
            ..TraceStatusResponse::default()
        }
    }

    async fn stop_trace(&mut self, _request: TraceStopRequest) -> TraceStopResponse {
        TraceStopResponse {
            status: -95,
            ..TraceStopResponse::default()
        }
    }

    async fn record_debug_event(&mut self, _name: &'static str) {}
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BufferedTraceManager {
    pub(super) active: Option<TraceSession>,
    pub(super) last_trace: Vec<u8>,
    pub(super) clock_ns: u64,
}

#[derive(Clone)]
pub struct TracedTraceManager {
    pub(super) channel: Channel,
    pub(super) last_trace: Vec<u8>,
    pub(super) output_format: u32,
}

impl TracedTraceManager {
    pub fn new(channel: Channel) -> Self {
        Self {
            channel,
            last_trace: Vec::new(),
            output_format: TraceOutputFormat::Perfetto.to_wire(),
        }
    }

    pub fn channel(&self) -> Channel {
        self.channel
    }

    fn call(
        &self,
        ordinal: u64,
        request: &[u8],
        handles: &[u64],
        response: &mut [u8],
        response_handles: &mut [u64],
    ) -> Result<(usize, usize), KernelStatus> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&ordinal.to_le_bytes());
        bytes.extend_from_slice(request);
        self.channel.send(&bytes, handles)?;
        for _ in 0..4096 {
            match self.channel.try_recv() {
                Ok(message) => {
                    let len = message.bytes.len().min(response.len());
                    response[..len].copy_from_slice(&message.bytes[..len]);
                    let handle_len = message.handles.len().min(response_handles.len());
                    response_handles[..handle_len].copy_from_slice(&message.handles[..handle_len]);
                    return Ok((len, handle_len));
                }
                Err(KernelStatus::ErrTimedOut) => bexos_userspace::yield_now(),
                Err(status) => return Err(status),
            }
        }
        Err(KernelStatus::ErrTimedOut)
    }

    fn chunk(&self, request: TraceStopRequest, status: i32) -> TraceStopResponse {
        let total_len = self.last_trace.len() as u64;
        let offset = request.offset.min(total_len);
        let max = if request.max_bytes == 0 {
            48 * 1024
        } else {
            request.max_bytes.min(48 * 1024)
        } as usize;
        let start = offset as usize;
        let end = start.saturating_add(max).min(self.last_trace.len());
        TraceStopResponse {
            status,
            offset,
            total_len,
            bytes: self.last_trace[start..end].to_vec(),
            complete: end == self.last_trace.len(),
            output_format: self.output_format,
        }
    }
}

impl BufferedTraceManager {
    pub fn new() -> Self {
        Self::default()
    }

    fn now(&mut self) -> u64 {
        self.clock_ns = self.clock_ns.saturating_add(1_000);
        self.clock_ns
    }

    fn current_status(&self, status: i32) -> TraceStatusResponse {
        let Some(session) = &self.active else {
            return TraceStatusResponse {
                status,
                state: trace_state_wire(TraceState::Idle),
                buffer_mode: BufferMode::CircularRing.to_wire(),
                output_format: bexos_trace::TraceOutputFormat::Perfetto.to_wire(),
                ..TraceStatusResponse::default()
            };
        };
        TraceStatusResponse {
            status,
            state: trace_state_wire(session.state()),
            categories: session.config().categories,
            buffer_mode: session.config().buffer_mode.to_wire(),
            buffer_size_kb: session.config().buffer_size_kb,
            output_format: session.config().output_format.to_wire(),
            producer_count: session.producer_count() as u32,
            event_count: session.event_count() as u64,
            dropped_count: session.dropped_count(),
        }
    }

    fn trace_chunk(&self, request: TraceStopRequest, status: i32) -> TraceStopResponse {
        let total_len = self.last_trace.len() as u64;
        let offset = request.offset.min(total_len);
        let max = if request.max_bytes == 0 {
            48 * 1024
        } else {
            request.max_bytes.min(48 * 1024)
        } as usize;
        let start = offset as usize;
        let end = start.saturating_add(max).min(self.last_trace.len());
        TraceStopResponse {
            status,
            offset,
            total_len,
            bytes: self.last_trace[start..end].to_vec(),
            complete: end == self.last_trace.len(),
            output_format: 1,
        }
    }
}

impl TraceManager for BufferedTraceManager {
    async fn start_trace(&mut self, request: TraceStartRequest) -> DebugStatusResponse {
        if self.active.is_some() {
            return debug_status(-7, "trace session already recording");
        }
        let Some(buffer_mode) = BufferMode::from_wire(request.buffer_mode) else {
            return invalid_request_response("invalid trace buffer mode");
        };
        let Some(output_format) = bexos_trace::TraceOutputFormat::from_wire(request.output_format)
        else {
            return invalid_request_response("invalid trace output format");
        };
        if request.categories == 0 || request.buffer_size_kb == 0 {
            return invalid_request_response("invalid trace session");
        }
        self.last_trace.clear();
        let mut session = TraceSession::new(
            TraceConfig {
                categories: request.categories,
                buffer_mode,
                buffer_size_kb: request.buffer_size_kb,
                output_format,
            },
            self.now(),
        );
        session.register_producer(
            bexos_trace::TraceProducer {
                id: 0,
                pid: 0,
                main_tid: 0,
                process_name: "debugd".into(),
                categories: CATEGORY_DEBUG_SERVICE,
            },
            self.now(),
        );
        session.record(TraceEvent::new(
            self.now(),
            0,
            0,
            CATEGORY_DEBUG_SERVICE,
            TraceEventKind::Instant,
            "debugd:trace_start",
        ));
        self.active = Some(session);
        ok_response("trace session started")
    }

    async fn trace_status(&mut self) -> TraceStatusResponse {
        self.current_status(0)
    }

    async fn stop_trace(&mut self, request: TraceStopRequest) -> TraceStopResponse {
        if request.offset == 0 {
            let Some(mut session) = self.active.take() else {
                if self.last_trace.is_empty() {
                    return TraceStopResponse {
                        status: -6,
                        ..TraceStopResponse::default()
                    };
                }
                return self.trace_chunk(request, 0);
            };
            session.record(TraceEvent::new(
                self.now(),
                0,
                0,
                CATEGORY_DEBUG_SERVICE,
                TraceEventKind::Instant,
                "debugd:trace_stop",
            ));
            self.last_trace = session.stop(self.now());
        }
        self.trace_chunk(request, 0)
    }

    async fn record_debug_event(&mut self, name: &'static str) {
        let timestamp = self.now();
        if let Some(session) = self.active.as_mut() {
            session.record(TraceEvent::new(
                timestamp,
                0,
                0,
                CATEGORY_DEBUG_SERVICE,
                TraceEventKind::Instant,
                name,
            ));
        }
    }
}

impl TraceManager for TracedTraceManager {
    async fn start_trace(&mut self, request: TraceStartRequest) -> DebugStatusResponse {
        let output_format = match TraceFidlOutputFormat::decode_value(request.output_format as u8) {
            Ok(format) => format,
            Err(_) => return invalid_request_response("invalid trace output format"),
        };
        let buffer_mode = match request.buffer_mode {
            1 => TraceFidlBufferMode::OneshotStopOnFull,
            2 => TraceFidlBufferMode::CircularRing,
            _ => return invalid_request_response("invalid trace buffer mode"),
        };
        let mut request_bytes = [0; 64];
        let mut request_handles = [TraceHandleRef { raw: 0 }; 1];
        let encoded = match (TraceControllerStartSessionRequest {
            categories: TraceCategory(request.categories),
            buffer_mode,
            buffer_size_kb: request.buffer_size_kb,
            output_format,
        })
        .encode(&mut request_bytes, &mut request_handles)
        {
            Ok(encoded) => encoded,
            Err(_) => return invalid_request_response("invalid TraceStart request"),
        };
        let mut response = [0; 64];
        let mut response_handles = [0; 1];
        let (bytes, handles) = match self.call(
            1,
            &request_bytes[..encoded.bytes],
            &[],
            &mut response,
            &mut response_handles,
        ) {
            Ok(result) => result,
            Err(status) => return debug_status(status as i32, "trace start failed"),
        };
        let refs = [TraceHandleRef {
            raw: response_handles[0],
        }];
        match TraceControllerStartSessionResponse::decode(&response[..bytes], &refs[..handles]) {
            Ok(response) if response.status == KernelStatus::Ok => {
                self.output_format = request.output_format.max(1);
                bexos_trace::trace_instant!(CATEGORY_DEBUG_SERVICE, "debugd:trace_start");
                ok_response("trace session started")
            }
            Ok(response) => debug_status(response.status as i32, "trace start failed"),
            Err(_) => invalid_request_response("invalid TraceStart response"),
        }
    }

    async fn trace_status(&mut self) -> TraceStatusResponse {
        let mut request_bytes = [0; 8];
        let mut request_handles = [TraceHandleRef { raw: 0 }; 1];
        let encoded = match (TraceControllerGetStatusRequest {})
            .encode(&mut request_bytes, &mut request_handles)
        {
            Ok(encoded) => encoded,
            Err(_) => {
                return TraceStatusResponse {
                    status: -8,
                    ..TraceStatusResponse::default()
                };
            }
        };
        let mut response = [0; 96];
        let mut response_handles = [0; 1];
        let (bytes, handles) = match self.call(
            3,
            &request_bytes[..encoded.bytes],
            &[],
            &mut response,
            &mut response_handles,
        ) {
            Ok(result) => result,
            Err(status) => {
                return TraceStatusResponse {
                    status: status as i32,
                    ..TraceStatusResponse::default()
                };
            }
        };
        let refs = [TraceHandleRef {
            raw: response_handles[0],
        }];
        match TraceControllerGetStatusResponse::decode(&response[..bytes], &refs[..handles]) {
            Ok(status) => {
                self.output_format = status.output_format as u32;
                TraceStatusResponse {
                    status: status.status as i32,
                    state: status.state as u32,
                    categories: status.categories.0,
                    buffer_mode: status.buffer_mode as u32,
                    buffer_size_kb: status.buffer_size_kb,
                    output_format: status.output_format as u32,
                    producer_count: status.producer_count,
                    event_count: status.event_count,
                    dropped_count: status.dropped_count,
                }
            }
            Err(_) => TraceStatusResponse {
                status: -8,
                ..TraceStatusResponse::default()
            },
        }
    }

    async fn stop_trace(&mut self, request: TraceStopRequest) -> TraceStopResponse {
        if request.offset == 0 {
            let mut request_bytes = [0; 8];
            let mut request_handles = [TraceHandleRef { raw: 0 }; 1];
            let encoded = match (TraceControllerStopSessionRequest {})
                .encode(&mut request_bytes, &mut request_handles)
            {
                Ok(encoded) => encoded,
                Err(_) => return self.chunk(request, -8),
            };
            let mut response = [0; 64];
            let mut response_handles = [0; 1];
            let (bytes, handles) = match self.call(
                2,
                &request_bytes[..encoded.bytes],
                &[],
                &mut response,
                &mut response_handles,
            ) {
                Ok(result) => result,
                Err(status) => return self.chunk(request, status as i32),
            };
            let refs = [TraceHandleRef {
                raw: response_handles[0],
            }];
            let response = match TraceControllerStopSessionResponse::decode(
                &response[..bytes],
                &refs[..handles],
            ) {
                Ok(response) => response,
                Err(_) => return self.chunk(request, -8),
            };
            if response.status != KernelStatus::Ok {
                return self.chunk(request, response.status as i32);
            }
            let len = response.trace_len as usize;
            self.last_trace.clear();
            if len > 0 && response.trace_file.raw != 0 {
                match Memory::map(response.trace_file.raw, response.trace_len, 2) {
                    Ok(addr) => {
                        let bytes = unsafe { core::slice::from_raw_parts(addr as *const u8, len) };
                        self.last_trace.extend_from_slice(bytes);
                        let _ = Memory::unmap(addr, response.trace_len);
                    }
                    Err(status) => return self.chunk(request, status as i32),
                }
                let _ = Memory::close(response.trace_file.raw);
            }
        }
        self.chunk(request, 0)
    }

    async fn record_debug_event(&mut self, name: &'static str) {
        bexos_trace::trace_instant!(CATEGORY_DEBUG_SERVICE, name);
    }
}

#[derive(Clone)]
pub enum LiveTraceManager {
    Proxy(TracedTraceManager),
    Buffered(BufferedTraceManager),
    Unsupported(UnsupportedTraceManager),
}

impl TraceManager for LiveTraceManager {
    async fn start_trace(&mut self, request: TraceStartRequest) -> DebugStatusResponse {
        match self {
            Self::Proxy(manager) => manager.start_trace(request).await,
            Self::Buffered(manager) => manager.start_trace(request).await,
            Self::Unsupported(manager) => manager.start_trace(request).await,
        }
    }

    async fn trace_status(&mut self) -> TraceStatusResponse {
        match self {
            Self::Proxy(manager) => manager.trace_status().await,
            Self::Buffered(manager) => manager.trace_status().await,
            Self::Unsupported(manager) => manager.trace_status().await,
        }
    }

    async fn stop_trace(&mut self, request: TraceStopRequest) -> TraceStopResponse {
        match self {
            Self::Proxy(manager) => manager.stop_trace(request).await,
            Self::Buffered(manager) => manager.stop_trace(request).await,
            Self::Unsupported(manager) => manager.stop_trace(request).await,
        }
    }

    async fn record_debug_event(&mut self, name: &'static str) {
        match self {
            Self::Proxy(manager) => manager.record_debug_event(name).await,
            Self::Buffered(manager) => manager.record_debug_event(name).await,
            Self::Unsupported(manager) => manager.record_debug_event(name).await,
        }
    }
}

pub(super) fn trace_state_wire(state: TraceState) -> u32 {
    match state {
        TraceState::Idle => 1,
        TraceState::Recording => 2,
        TraceState::Stopped => 3,
    }
}
