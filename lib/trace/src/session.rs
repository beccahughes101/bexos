use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::{
    CATEGORY_ALL, CATEGORY_DEBUG_SERVICE, TraceEvent, TraceEventKind, export_legacy_bexos_fxt,
    export_perfetto_trace,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BufferMode {
    OneshotStopOnFull,
    CircularRing,
}

impl Default for BufferMode {
    fn default() -> Self {
        Self::CircularRing
    }
}

impl BufferMode {
    pub fn from_wire(value: u32) -> Option<Self> {
        match value {
            1 => Some(Self::OneshotStopOnFull),
            2 => Some(Self::CircularRing),
            _ => None,
        }
    }

    pub const fn to_wire(self) -> u32 {
        match self {
            Self::OneshotStopOnFull => 1,
            Self::CircularRing => 2,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TraceOutputFormat {
    Perfetto,
    LegacyBexosFxt,
}

impl Default for TraceOutputFormat {
    fn default() -> Self {
        Self::Perfetto
    }
}

impl TraceOutputFormat {
    pub const fn from_wire(value: u32) -> Option<Self> {
        match value {
            1 => Some(Self::Perfetto),
            2 => Some(Self::LegacyBexosFxt),
            _ => None,
        }
    }

    pub const fn to_wire(self) -> u32 {
        match self {
            Self::Perfetto => 1,
            Self::LegacyBexosFxt => 2,
        }
    }

    pub const fn extension(self) -> &'static str {
        match self {
            Self::Perfetto => "pftrace",
            Self::LegacyBexosFxt => "fxt",
        }
    }

    pub fn export(self, events: &[TraceEvent]) -> Vec<u8> {
        match self {
            Self::Perfetto => export_perfetto_trace(events),
            Self::LegacyBexosFxt => export_legacy_bexos_fxt(events),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TraceState {
    Idle,
    Recording,
    Stopped,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraceProducer {
    pub id: u64,
    pub pid: u64,
    pub main_tid: u64,
    pub process_name: String,
    pub categories: u32,
}

impl TraceProducer {
    pub fn new(pid: u64, process_name: &str, categories: u32) -> Self {
        Self {
            id: pid,
            pid,
            main_tid: 0,
            process_name: process_name.to_string(),
            categories,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraceConfig {
    pub categories: u32,
    pub buffer_mode: BufferMode,
    pub buffer_size_kb: u32,
    pub output_format: TraceOutputFormat,
}

impl Default for TraceConfig {
    fn default() -> Self {
        Self {
            categories: CATEGORY_ALL,
            buffer_mode: BufferMode::CircularRing,
            buffer_size_kb: 2048,
            output_format: TraceOutputFormat::Perfetto,
        }
    }
}

impl TraceConfig {
    pub fn capacity_events(&self) -> usize {
        let bytes = (self.buffer_size_kb as usize).saturating_mul(1024);
        (bytes / 64).clamp(16, 65536)
    }

    pub fn accepts(&self, category: u32) -> bool {
        category == 0 || self.categories & category != 0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraceBuffer {
    mode: BufferMode,
    capacity: usize,
    events: Vec<TraceEvent>,
    dropped: u64,
    stopped_on_full: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraceBufferSnapshot {
    pub mode: BufferMode,
    pub capacity: usize,
    pub events: Vec<TraceEvent>,
    pub dropped: u64,
    pub stopped_on_full: bool,
}

impl TraceBuffer {
    pub fn new(config: &TraceConfig) -> Self {
        Self {
            mode: config.buffer_mode,
            capacity: config.capacity_events(),
            events: Vec::new(),
            dropped: 0,
            stopped_on_full: false,
        }
    }

    pub fn push(&mut self, event: TraceEvent) -> bool {
        if self.stopped_on_full {
            self.dropped = self.dropped.saturating_add(1);
            return false;
        }
        if self.events.len() < self.capacity {
            self.events.push(event);
            return true;
        }
        self.dropped = self.dropped.saturating_add(1);
        match self.mode {
            BufferMode::OneshotStopOnFull => {
                self.stopped_on_full = true;
                false
            }
            BufferMode::CircularRing => {
                if !self.events.is_empty() {
                    self.events.remove(0);
                    self.events.push(event);
                    true
                } else {
                    false
                }
            }
        }
    }

    pub fn events(&self) -> &[TraceEvent] {
        &self.events
    }

    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    pub fn stopped_on_full(&self) -> bool {
        self.stopped_on_full
    }

    pub fn snapshot(&self) -> TraceBufferSnapshot {
        TraceBufferSnapshot {
            mode: self.mode,
            capacity: self.capacity,
            events: self.events.clone(),
            dropped: self.dropped,
            stopped_on_full: self.stopped_on_full,
        }
    }

    pub fn from_snapshot(snapshot: TraceBufferSnapshot) -> Self {
        Self {
            mode: snapshot.mode,
            capacity: snapshot.capacity,
            events: snapshot.events,
            dropped: snapshot.dropped,
            stopped_on_full: snapshot.stopped_on_full,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraceSession {
    config: TraceConfig,
    state: TraceState,
    producers: Vec<TraceProducer>,
    buffer: TraceBuffer,
    started_ns: u64,
    stopped_ns: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraceSessionSnapshot {
    pub config: TraceConfig,
    pub state: TraceState,
    pub producers: Vec<TraceProducer>,
    pub buffer: TraceBufferSnapshot,
    pub started_ns: u64,
    pub stopped_ns: u64,
}

impl TraceSession {
    pub fn new(config: TraceConfig, timestamp_ns: u64) -> Self {
        let mut buffer = TraceBuffer::new(&config);
        buffer.push(TraceEvent::metadata(timestamp_ns, "traced:session_start"));
        Self {
            config,
            state: TraceState::Recording,
            producers: Vec::new(),
            buffer,
            started_ns: timestamp_ns,
            stopped_ns: 0,
        }
    }

    pub fn config(&self) -> &TraceConfig {
        &self.config
    }

    pub fn state(&self) -> TraceState {
        self.state
    }

    pub fn started_ns(&self) -> u64 {
        self.started_ns
    }

    pub fn stopped_ns(&self) -> u64 {
        self.stopped_ns
    }

    pub fn producer_count(&self) -> usize {
        self.producers.len()
    }

    pub fn producers(&self) -> &[TraceProducer] {
        &self.producers
    }

    pub fn event_count(&self) -> usize {
        self.buffer.events().len()
    }

    pub fn dropped_count(&self) -> u64 {
        self.buffer.dropped()
    }

    pub fn events(&self) -> &[TraceEvent] {
        self.buffer.events()
    }

    pub fn register_producer(&mut self, producer: TraceProducer, timestamp_ns: u64) {
        if let Some(existing) = self
            .producers
            .iter_mut()
            .find(|entry| entry.id == producer.id)
        {
            *existing = producer;
        } else {
            self.producers.push(producer);
        }
        self.record(TraceEvent::metadata(
            timestamp_ns,
            "traced:producer_registered",
        ));
    }

    pub fn unregister_producer(&mut self, producer_id: u64, timestamp_ns: u64) {
        self.producers.retain(|producer| producer.id != producer_id);
        self.record(
            TraceEvent::metadata(timestamp_ns, "traced:producer_unregistered").annotation(
                0,
                "producer_id",
                producer_id as i64,
            ),
        );
    }

    pub fn record(&mut self, event: TraceEvent) -> bool {
        if self.state != TraceState::Recording || !self.config.accepts(event.category) {
            return false;
        }
        self.buffer.push(event)
    }

    pub fn stop(&mut self, timestamp_ns: u64) -> Vec<u8> {
        self.stop_with_format(timestamp_ns, self.config.output_format)
    }

    pub fn stop_with_format(&mut self, timestamp_ns: u64, format: TraceOutputFormat) -> Vec<u8> {
        if self.state == TraceState::Recording {
            self.buffer
                .push(TraceEvent::metadata(timestamp_ns, "traced:session_stop"));
            if self.buffer.dropped() > 0 {
                self.buffer.push(
                    TraceEvent::new(
                        timestamp_ns,
                        0,
                        0,
                        CATEGORY_DEBUG_SERVICE,
                        TraceEventKind::DroppedEvents,
                        "traced:dropped_events",
                    )
                    .counter(self.buffer.dropped() as i64),
                );
            }
            self.state = TraceState::Stopped;
            self.stopped_ns = timestamp_ns;
        }
        format.export(self.buffer.events())
    }

    pub fn snapshot(&self) -> TraceSessionSnapshot {
        TraceSessionSnapshot {
            config: self.config.clone(),
            state: self.state,
            producers: self.producers.clone(),
            buffer: self.buffer.snapshot(),
            started_ns: self.started_ns,
            stopped_ns: self.stopped_ns,
        }
    }

    pub fn from_snapshot(snapshot: TraceSessionSnapshot) -> Self {
        Self {
            config: snapshot.config,
            state: snapshot.state,
            producers: snapshot.producers,
            buffer: TraceBuffer::from_snapshot(snapshot.buffer),
            started_ns: snapshot.started_ns,
            stopped_ns: snapshot.stopped_ns,
        }
    }
}
