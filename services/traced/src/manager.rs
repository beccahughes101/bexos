use alloc::string::ToString;
use alloc::vec::Vec;
use bexos_trace::{
    BufferMode, CATEGORY_DEBUG_SERVICE, SharedTraceReader, SharedTraceWriter, TraceConfig,
    TraceEvent, TraceEventKind, TraceOutputFormat, TraceProducer, TraceSession,
    TraceSessionSnapshot, TraceState,
};

pub const DEFAULT_BUFFER_SIZE_KB: u32 = 2048;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraceStatus {
    pub state: TraceState,
    pub categories: u32,
    pub buffer_mode: BufferMode,
    pub buffer_size_kb: u32,
    pub output_format: TraceOutputFormat,
    pub producer_count: u32,
    pub event_count: u64,
    pub dropped_count: u64,
}

impl Default for TraceStatus {
    fn default() -> Self {
        Self {
            state: TraceState::Idle,
            categories: 0,
            buffer_mode: BufferMode::CircularRing,
            buffer_size_kb: DEFAULT_BUFFER_SIZE_KB,
            output_format: TraceOutputFormat::Perfetto,
            producer_count: 0,
            event_count: 0,
            dropped_count: 0,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraceManager {
    active: Option<TraceSession>,
    producers: Vec<RegisteredProducer>,
    last: TraceStatus,
    last_trace: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraceManagerSnapshot {
    pub active: Option<TraceSessionSnapshot>,
    pub producers: Vec<RegisteredProducerSnapshot>,
    pub last: TraceStatus,
    pub last_trace: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegisteredProducer {
    pub producer: TraceProducer,
    pub vmo: u64,
    pub mapped_addr: u64,
    pub mapped_len: u64,
    pub cursor: u64,
    pub dropped_seen: u64,
    pub writer: Option<SharedTraceWriter>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegisteredProducerSnapshot {
    pub producer: TraceProducer,
    pub vmo: u64,
    pub mapped_addr: u64,
    pub mapped_len: u64,
    pub cursor: u64,
    pub dropped_seen: u64,
}

impl TraceManager {
    pub fn new() -> Self {
        Self {
            active: None,
            producers: Vec::new(),
            last: TraceStatus::default(),
            last_trace: Vec::new(),
        }
    }

    pub fn start(
        &mut self,
        categories: u32,
        buffer_mode: BufferMode,
        buffer_size_kb: u32,
        timestamp_ns: u64,
    ) -> Result<(), TraceError> {
        self.start_with_format(
            categories,
            buffer_mode,
            buffer_size_kb,
            TraceOutputFormat::Perfetto,
            timestamp_ns,
        )
    }

    pub fn start_with_format(
        &mut self,
        categories: u32,
        buffer_mode: BufferMode,
        buffer_size_kb: u32,
        output_format: TraceOutputFormat,
        timestamp_ns: u64,
    ) -> Result<(), TraceError> {
        if self
            .active
            .as_ref()
            .is_some_and(|session| session.state() == TraceState::Recording)
        {
            return Err(TraceError::AlreadyRecording);
        }
        if categories == 0 || buffer_size_kb == 0 {
            return Err(TraceError::InvalidArgs);
        }
        let config = TraceConfig {
            categories,
            buffer_mode,
            buffer_size_kb,
            output_format,
        };
        let mut session = TraceSession::new(config, timestamp_ns);
        session.register_producer(
            TraceProducer {
                id: 0,
                pid: 0,
                main_tid: 0,
                process_name: "traced".to_string(),
                categories: CATEGORY_DEBUG_SERVICE,
            },
            timestamp_ns.saturating_add(1),
        );
        for registered in &mut self.producers {
            registered.cursor = 0;
            registered.dropped_seen = 0;
            if let Some(writer) = registered.writer {
                writer.configure(session.config(), timestamp_ns);
            }
            session.register_producer(registered.producer.clone(), timestamp_ns.saturating_add(1));
        }
        session.record(TraceEvent::new(
            timestamp_ns.saturating_add(2),
            0,
            0,
            CATEGORY_DEBUG_SERVICE,
            TraceEventKind::Instant,
            "traced:ready",
        ));
        self.last_trace.clear();
        self.last = status_from_session(&session, self.producers.len() as u32);
        self.active = Some(session);
        Ok(())
    }

    pub fn register_producer(
        &mut self,
        pid: u64,
        process_name: &str,
        categories: u32,
        timestamp_ns: u64,
    ) -> Result<(), TraceError> {
        if pid == 0 || process_name.is_empty() || categories == 0 || process_name.len() > 64 {
            return Err(TraceError::InvalidArgs);
        }
        self.register_metadata_producer(
            TraceProducer::new(pid, process_name, categories),
            timestamp_ns,
        )
    }

    pub fn register_metadata_producer(
        &mut self,
        producer: TraceProducer,
        timestamp_ns: u64,
    ) -> Result<(), TraceError> {
        if producer.id == 0
            || producer.pid == 0
            || producer.process_name.is_empty()
            || producer.categories == 0
            || producer.process_name.len() > 64
        {
            return Err(TraceError::InvalidArgs);
        }
        if self
            .producers
            .iter()
            .any(|entry| entry.producer.id == producer.id)
        {
            return Err(TraceError::AlreadyExists);
        }
        self.producers.push(RegisteredProducer {
            producer: producer.clone(),
            vmo: 0,
            mapped_addr: 0,
            mapped_len: 0,
            cursor: 0,
            dropped_seen: 0,
            writer: None,
        });
        if let Some(session) = self.active.as_mut() {
            session.register_producer(producer, timestamp_ns);
            self.last = status_from_session(session, self.producers.len() as u32);
        } else {
            self.last.producer_count = self.producers.len() as u32;
        }
        Ok(())
    }

    pub fn register_shared_producer(
        &mut self,
        producer: TraceProducer,
        vmo: u64,
        mapped_addr: u64,
        mapped_len: u64,
        timestamp_ns: u64,
    ) -> Result<(), TraceError> {
        if producer.id == 0
            || producer.pid == 0
            || producer.process_name.is_empty()
            || producer.categories == 0
            || producer.process_name.len() > 64
            || mapped_addr == 0
            || mapped_len == 0
        {
            return Err(TraceError::InvalidArgs);
        }
        if self
            .producers
            .iter()
            .any(|entry| entry.producer.id == producer.id)
        {
            return Err(TraceError::AlreadyExists);
        }
        let writer = unsafe {
            SharedTraceWriter::from_raw_parts(
                mapped_addr as *mut u8,
                mapped_len as usize,
                producer.id,
                producer.pid,
                producer.main_tid,
            )
        };
        if writer.is_none() {
            return Err(TraceError::InvalidArgs);
        }
        if let (Some(session), Some(writer)) = (self.active.as_mut(), writer) {
            writer.configure(session.config(), timestamp_ns);
            session.register_producer(producer.clone(), timestamp_ns);
        }
        self.producers.push(RegisteredProducer {
            producer,
            vmo,
            mapped_addr,
            mapped_len,
            cursor: 0,
            dropped_seen: 0,
            writer,
        });
        self.refresh_status();
        Ok(())
    }

    pub fn replace_shared_producer(
        &mut self,
        producer: TraceProducer,
        vmo: u64,
        mapped_addr: u64,
        mapped_len: u64,
        timestamp_ns: u64,
    ) -> Result<Option<u64>, TraceError> {
        let old_vmo = self.unregister_producer(producer.id, timestamp_ns)?;
        self.register_shared_producer(producer, vmo, mapped_addr, mapped_len, timestamp_ns)?;
        Ok(old_vmo)
    }

    pub fn unregister_producer(
        &mut self,
        producer_id: u64,
        timestamp_ns: u64,
    ) -> Result<Option<u64>, TraceError> {
        let Some(index) = self
            .producers
            .iter()
            .position(|entry| entry.producer.id == producer_id)
        else {
            return Err(TraceError::NotFound);
        };
        let producer = self.producers.remove(index);
        if let Some(writer) = producer.writer {
            writer.disable();
        }
        if let Some(session) = self.active.as_mut() {
            session.unregister_producer(producer_id, timestamp_ns);
        }
        self.refresh_status();
        Ok((producer.vmo != 0).then_some(producer.vmo))
    }

    pub fn record_debug_event(&mut self, name: &str, timestamp_ns: u64) {
        if let Some(session) = self.active.as_mut() {
            session.record(TraceEvent::new(
                timestamp_ns,
                0,
                0,
                CATEGORY_DEBUG_SERVICE,
                TraceEventKind::Instant,
                name,
            ));
            self.last = status_from_session(session, self.producers.len() as u32);
        }
    }

    pub fn stop(&mut self, timestamp_ns: u64) -> Result<Vec<u8>, TraceError> {
        self.drain_shared();
        let Some(mut session) = self.active.take() else {
            return Err(TraceError::NotRecording);
        };
        let trace = session.stop(timestamp_ns);
        self.last = status_from_session(&session, self.producers.len() as u32);
        self.last_trace = trace.clone();
        Ok(trace)
    }

    pub fn status(&self) -> TraceStatus {
        self.active
            .as_ref()
            .map(|session| status_from_session(session, self.producers.len() as u32))
            .unwrap_or_else(|| self.last.clone())
    }

    pub fn refresh_status(&mut self) {
        self.drain_shared();
        if let Some(session) = self.active.as_ref() {
            self.last = status_from_session(session, self.producers.len() as u32);
        } else {
            self.last.producer_count = self.producers.len() as u32;
        }
    }

    pub fn last_trace(&self) -> &[u8] {
        &self.last_trace
    }

    pub fn snapshot(&self) -> TraceManagerSnapshot {
        TraceManagerSnapshot {
            active: self.active.as_ref().map(TraceSession::snapshot),
            producers: self
                .producers
                .iter()
                .map(|producer| RegisteredProducerSnapshot {
                    producer: producer.producer.clone(),
                    vmo: producer.vmo,
                    mapped_addr: producer.mapped_addr,
                    mapped_len: producer.mapped_len,
                    cursor: producer.cursor,
                    dropped_seen: producer.dropped_seen,
                })
                .collect(),
            last: self.last.clone(),
            last_trace: self.last_trace.clone(),
        }
    }

    pub fn from_snapshot(snapshot: TraceManagerSnapshot) -> Self {
        Self {
            active: snapshot.active.map(TraceSession::from_snapshot),
            producers: snapshot
                .producers
                .into_iter()
                .map(|producer| RegisteredProducer {
                    writer: unsafe {
                        SharedTraceWriter::from_raw_parts(
                            producer.mapped_addr as *mut u8,
                            producer.mapped_len as usize,
                            producer.producer.id,
                            producer.producer.pid,
                            producer.producer.main_tid,
                        )
                    },
                    producer: producer.producer,
                    vmo: producer.vmo,
                    mapped_addr: producer.mapped_addr,
                    mapped_len: producer.mapped_len,
                    cursor: producer.cursor,
                    dropped_seen: producer.dropped_seen,
                })
                .collect(),
            last: snapshot.last,
            last_trace: snapshot.last_trace,
        }
    }

    fn drain_shared(&mut self) {
        let Some(session) = self.active.as_mut() else {
            return;
        };
        for registered in &mut self.producers {
            if registered.mapped_addr == 0 || registered.mapped_len == 0 {
                continue;
            }
            let Some(reader) = (unsafe {
                SharedTraceReader::from_raw_parts(
                    registered.mapped_addr as *const u8,
                    registered.mapped_len as usize,
                )
            }) else {
                continue;
            };
            for event in reader.drain_since(&mut registered.cursor) {
                session.record(event);
            }
            let dropped = reader.dropped_count();
            if dropped > registered.dropped_seen {
                let delta = dropped - registered.dropped_seen;
                registered.dropped_seen = dropped;
                session.record(
                    TraceEvent::new(
                        0,
                        registered.producer.pid,
                        registered.producer.main_tid,
                        CATEGORY_DEBUG_SERVICE,
                        TraceEventKind::DroppedEvents,
                        "traced:producer_dropped_events",
                    )
                    .counter(delta as i64)
                    .annotation(
                        0,
                        "producer_id",
                        registered.producer.id as i64,
                    ),
                );
            }
        }
    }
}

impl Default for TraceManager {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TraceError {
    InvalidArgs,
    AlreadyRecording,
    AlreadyExists,
    NotRecording,
    NotFound,
}

fn status_from_session(session: &TraceSession, registered_producers: u32) -> TraceStatus {
    TraceStatus {
        state: session.state(),
        categories: session.config().categories,
        buffer_mode: session.config().buffer_mode,
        buffer_size_kb: session.config().buffer_size_kb,
        output_format: session.config().output_format,
        producer_count: (session.producer_count() as u32).max(registered_producers),
        event_count: session.event_count() as u64,
        dropped_count: session.dropped_count(),
    }
}
