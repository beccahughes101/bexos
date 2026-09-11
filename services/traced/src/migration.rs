use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_trace::{
    BufferMode, TraceBufferSnapshot, TraceConfig, TraceEvent, TraceEventKind, TraceProducer,
    TraceSessionSnapshot, TraceState,
};
use bexos_userspace::Channel;
use bexos_userspace::live_migration::{Resource, State};
use bexos_userspace::service_binding::BoundServiceEndpoint;

use crate::manager::{TraceManager, TraceStatus};

const KEY_HEADER: u64 = 0;
const KEY_CLIENTS: u64 = 1;
const KEY_MANAGER: u64 = 2;
const KEY_ACTIVE_EVENTS_BASE: u64 = 1000;
const KEY_LAST_TRACE_BASE: u64 = 100_000;
const MAX_EVENTS_PER_RECORD: usize = 128;
const MAX_TRACE_CHUNK: usize = 32 * 1024;

pub struct Runtime {
    pub control: Channel,
    pub migration: Option<Channel>,
    pub clients: Vec<BoundServiceEndpoint>,
    pub manager: TraceManager,
}

impl Runtime {
    pub fn new(control: Channel, migration: Option<Channel>, manager: TraceManager) -> Self {
        Self {
            control,
            migration,
            clients: Vec::new(),
            manager,
        }
    }
}

impl State for Runtime {
    fn empty() -> Self {
        Self {
            control: Channel(0),
            migration: None,
            clients: Vec::new(),
            manager: TraceManager::new(),
        }
    }

    fn keys(&self) -> Vec<u64> {
        let snapshot = self.manager.snapshot();
        let mut keys = vec![KEY_HEADER, KEY_CLIENTS, KEY_MANAGER];
        if let Some(active) = &snapshot.active {
            let count = active.buffer.events.len().div_ceil(MAX_EVENTS_PER_RECORD);
            keys.extend((0..count).map(|index| KEY_ACTIVE_EVENTS_BASE + index as u64));
        }
        let count = snapshot.last_trace.len().div_ceil(MAX_TRACE_CHUNK);
        keys.extend((0..count).map(|index| KEY_LAST_TRACE_BASE + index as u64));
        keys
    }

    fn encode_record(&self, key: u64) -> Result<Option<Vec<u8>>, Error> {
        let snapshot = self.manager.snapshot();
        match key {
            KEY_HEADER => {
                let mut w = Encoder::new();
                w.word(1);
                w.word(self.control.0);
                w.word(self.migration.map_or(0, |channel| channel.0));
                Ok(Some(w.finish()))
            }
            KEY_CLIENTS => {
                let mut w = Encoder::new();
                w.word(2);
                w.word(self.clients.len() as u64);
                for client in &self.clients {
                    w.word(client.channel.0);
                    w.text(&client.protocol);
                    w.word(client.allowed_methods.len() as u64);
                    for ordinal in &client.allowed_methods {
                        w.word(*ordinal);
                    }
                }
                Ok(Some(w.finish()))
            }
            KEY_MANAGER => {
                let mut w = Encoder::new();
                w.word(2);
                w.flag(snapshot.active.is_some());
                if let Some(active) = &snapshot.active {
                    encode_session_header(&mut w, active);
                }
                encode_status(&mut w, &snapshot.last);
                w.word(snapshot.last_trace.len() as u64);
                w.word(snapshot.producers.len() as u64);
                for producer in &snapshot.producers {
                    encode_producer(&mut w, &producer.producer);
                    w.word(producer.vmo);
                    w.word(producer.mapped_addr);
                    w.word(producer.mapped_len);
                    w.word(producer.cursor);
                    w.word(producer.dropped_seen);
                }
                Ok(Some(w.finish()))
            }
            key if (KEY_ACTIVE_EVENTS_BASE..KEY_LAST_TRACE_BASE).contains(&key) => {
                let Some(active) = &snapshot.active else {
                    return Ok(None);
                };
                let index =
                    usize::try_from(key - KEY_ACTIVE_EVENTS_BASE).map_err(|_| Error::Capacity)?;
                let start = index
                    .checked_mul(MAX_EVENTS_PER_RECORD)
                    .ok_or(Error::Capacity)?;
                if start >= active.buffer.events.len() {
                    return Ok(None);
                }
                let end = start
                    .saturating_add(MAX_EVENTS_PER_RECORD)
                    .min(active.buffer.events.len());
                let mut w = Encoder::new();
                w.word(index as u64);
                w.word(active.buffer.events.len() as u64);
                w.word((end - start) as u64);
                for event in &active.buffer.events[start..end] {
                    encode_event(&mut w, event);
                }
                Ok(Some(w.finish()))
            }
            key if key >= KEY_LAST_TRACE_BASE => {
                let index =
                    usize::try_from(key - KEY_LAST_TRACE_BASE).map_err(|_| Error::Capacity)?;
                let start = index.checked_mul(MAX_TRACE_CHUNK).ok_or(Error::Capacity)?;
                if start >= snapshot.last_trace.len() {
                    return Ok(None);
                }
                let end = start
                    .saturating_add(MAX_TRACE_CHUNK)
                    .min(snapshot.last_trace.len());
                let mut w = Encoder::new();
                w.word(index as u64);
                w.word(snapshot.last_trace.len() as u64);
                w.bytes(&snapshot.last_trace[start..end]);
                Ok(Some(w.finish()))
            }
            _ => Ok(None),
        }
    }

    fn adopt_record(&mut self, key: u64, bytes: Option<&[u8]>) -> Result<(), Error> {
        match key {
            KEY_HEADER => {
                let mut r = Decoder::new(bytes.ok_or(Error::InvalidData)?);
                if r.word()? != 1 {
                    return Err(Error::UnsupportedVersion);
                }
                self.control = Channel(r.word()?);
                let migration = r.word()?;
                self.migration = (migration != 0).then_some(Channel(migration));
                r.finish()
            }
            KEY_CLIENTS => {
                let mut r = Decoder::new(bytes.ok_or(Error::InvalidData)?);
                let version = r.word()?;
                if !(version == 1 || version == 2) {
                    return Err(Error::UnsupportedVersion);
                }
                self.clients.clear();
                for _ in 0..r.count(256)? {
                    let channel = Channel(r.word()?);
                    let protocol = if version >= 2 {
                        r.text(64)?.to_string()
                    } else {
                        String::new()
                    };
                    let mut allowed_methods = Vec::new();
                    for _ in 0..r.count(64)? {
                        allowed_methods.push(r.word()?);
                    }
                    self.clients.push(BoundServiceEndpoint::new_with_protocol(
                        channel,
                        allowed_methods,
                        &protocol,
                    ));
                }
                r.finish()
            }
            KEY_MANAGER => {
                let mut r = Decoder::new(bytes.ok_or(Error::InvalidData)?);
                let version = r.word()?;
                if !(version == 1 || version == 2) {
                    return Err(Error::UnsupportedVersion);
                }
                let active = if r.flag()? {
                    Some(decode_session_header(&mut r)?)
                } else {
                    None
                };
                let last = decode_status(&mut r)?;
                let last_trace_len = usize::try_from(r.word()?).map_err(|_| Error::Capacity)?;
                let producers = if version >= 2 {
                    let mut producers = Vec::new();
                    for _ in 0..r.count(512)? {
                        producers.push(crate::manager::RegisteredProducerSnapshot {
                            producer: decode_producer(&mut r)?,
                            vmo: r.word()?,
                            mapped_addr: r.word()?,
                            mapped_len: r.word()?,
                            cursor: r.word()?,
                            dropped_seen: r.word()?,
                        });
                    }
                    producers
                } else {
                    Vec::new()
                };
                r.finish()?;
                let mut snapshot = self.manager.snapshot();
                snapshot.active = active;
                snapshot.producers = producers;
                snapshot.last = last;
                snapshot.last_trace.clear();
                snapshot.last_trace.resize(last_trace_len, 0);
                self.manager = TraceManager::from_snapshot(snapshot);
                Ok(())
            }
            key if (KEY_ACTIVE_EVENTS_BASE..KEY_LAST_TRACE_BASE).contains(&key) => {
                let bytes = bytes.ok_or(Error::InvalidData)?;
                let mut r = Decoder::new(bytes);
                let index = usize::try_from(r.word()?).map_err(|_| Error::Capacity)?;
                let total = usize::try_from(r.word()?).map_err(|_| Error::Capacity)?;
                let count = r.count(MAX_EVENTS_PER_RECORD)?;
                let mut snapshot = self.manager.snapshot();
                let active = snapshot.active.as_mut().ok_or(Error::InvalidData)?;
                active.buffer.events.resize(
                    total,
                    TraceEvent::metadata(0, "traced:migration_placeholder"),
                );
                let start = index
                    .checked_mul(MAX_EVENTS_PER_RECORD)
                    .ok_or(Error::Capacity)?;
                let end = start.checked_add(count).ok_or(Error::Capacity)?;
                if end > total {
                    return Err(Error::InvalidData);
                }
                for slot in &mut active.buffer.events[start..end] {
                    *slot = decode_event(&mut r)?;
                }
                r.finish()?;
                self.manager = TraceManager::from_snapshot(snapshot);
                Ok(())
            }
            key if key >= KEY_LAST_TRACE_BASE => {
                let mut r = Decoder::new(bytes.ok_or(Error::InvalidData)?);
                let index = usize::try_from(r.word()?).map_err(|_| Error::Capacity)?;
                let total = usize::try_from(r.word()?).map_err(|_| Error::Capacity)?;
                let chunk = r.bytes(MAX_TRACE_CHUNK)?;
                let start = index.checked_mul(MAX_TRACE_CHUNK).ok_or(Error::Capacity)?;
                let end = start.checked_add(chunk.len()).ok_or(Error::Capacity)?;
                if end > total {
                    return Err(Error::InvalidData);
                }
                let mut snapshot = self.manager.snapshot();
                snapshot.last_trace.resize(total, 0);
                snapshot.last_trace[start..end].copy_from_slice(chunk);
                r.finish()?;
                self.manager = TraceManager::from_snapshot(snapshot);
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn validate(&self) -> Result<(), Error> {
        if self.control.0 == 0 || self.migration.is_none() {
            return Err(Error::InvalidData);
        }
        if self.clients.iter().any(|client| client.channel.0 == 0) {
            return Err(Error::InvalidData);
        }
        Ok(())
    }

    fn resources(&self) -> Vec<Resource> {
        let mut handles = vec![self.control.0];
        if let Some(migration) = self.migration {
            handles.push(migration.0);
        }
        handles.extend(self.clients.iter().map(|client| client.channel.0));
        handles.extend(
            self.manager
                .snapshot()
                .producers
                .iter()
                .filter_map(|producer| (producer.vmo != 0).then_some(producer.vmo)),
        );
        handles.into_iter().map(Resource::Handle).collect()
    }

    fn activated(&mut self, _generation: u64) {}
}

trait EncoderExt {
    fn flag(&mut self, value: bool);
}

impl EncoderExt for Encoder {
    fn flag(&mut self, value: bool) {
        self.word(value as u64);
    }
}

fn encode_status(w: &mut Encoder, status: &TraceStatus) {
    w.word(trace_state_to_wire(status.state));
    w.word(buffer_mode_to_wire(status.buffer_mode));
    w.word(status.categories as u64);
    w.word(status.buffer_size_kb as u64);
    w.word(status.output_format.to_wire() as u64);
    w.word(status.producer_count as u64);
    w.word(status.event_count);
    w.word(status.dropped_count);
}

fn decode_status(r: &mut Decoder<'_>) -> Result<TraceStatus, Error> {
    Ok(TraceStatus {
        state: trace_state_from_wire(r.word()?)?,
        buffer_mode: buffer_mode_from_wire(r.word()?)?,
        categories: r.word()? as u32,
        buffer_size_kb: r.word()? as u32,
        output_format: bexos_trace::TraceOutputFormat::from_wire(r.word()? as u32)
            .ok_or(Error::InvalidData)?,
        producer_count: r.word()? as u32,
        event_count: r.word()?,
        dropped_count: r.word()?,
    })
}

fn encode_session_header(w: &mut Encoder, session: &TraceSessionSnapshot) {
    w.word(session.config.categories as u64);
    w.word(buffer_mode_to_wire(session.config.buffer_mode));
    w.word(session.config.buffer_size_kb as u64);
    w.word(session.config.output_format.to_wire() as u64);
    w.word(trace_state_to_wire(session.state));
    w.word(session.started_ns);
    w.word(session.stopped_ns);
    w.word(session.producers.len() as u64);
    for producer in &session.producers {
        encode_producer(w, producer);
    }
    w.word(session.buffer.capacity as u64);
    w.word(session.buffer.dropped);
    w.flag(session.buffer.stopped_on_full);
}

fn decode_session_header(r: &mut Decoder<'_>) -> Result<TraceSessionSnapshot, Error> {
    let config = TraceConfig {
        categories: r.word()? as u32,
        buffer_mode: buffer_mode_from_wire(r.word()?)?,
        buffer_size_kb: r.word()? as u32,
        output_format: bexos_trace::TraceOutputFormat::from_wire(r.word()? as u32)
            .ok_or(Error::InvalidData)?,
    };
    let state = trace_state_from_wire(r.word()?)?;
    let started_ns = r.word()?;
    let stopped_ns = r.word()?;
    let mut producers = Vec::new();
    for _ in 0..r.count(256)? {
        producers.push(decode_producer(r)?);
    }
    let capacity = usize::try_from(r.word()?).map_err(|_| Error::Capacity)?;
    let dropped = r.word()?;
    let stopped_on_full = r.flag()?;
    Ok(TraceSessionSnapshot {
        config: config.clone(),
        state,
        producers,
        buffer: TraceBufferSnapshot {
            mode: config.buffer_mode,
            capacity,
            events: Vec::new(),
            dropped,
            stopped_on_full,
        },
        started_ns,
        stopped_ns,
    })
}

fn encode_event(w: &mut Encoder, event: &TraceEvent) {
    w.word(event.timestamp_ns);
    w.word(event.pid);
    w.word(event.tid);
    w.word(event.category as u64);
    w.word(event_kind_to_wire(event.kind));
    w.text(&event.name);
    w.word(event.flow_id);
    w.word(event.value as u64);
    w.word(event.annotations.iter().filter(|a| a.is_some()).count() as u64);
    for annotation in event.annotations.iter().flatten() {
        w.text(&annotation.key);
        w.word(annotation.value as u64);
    }
}

fn decode_event(r: &mut Decoder<'_>) -> Result<TraceEvent, Error> {
    let mut event = TraceEvent {
        timestamp_ns: r.word()?,
        pid: r.word()?,
        tid: r.word()?,
        category: r.word()? as u32,
        kind: event_kind_from_wire(r.word()?)?,
        name: r.text(256)?.to_string(),
        flow_id: r.word()?,
        value: r.word()? as i64,
        annotations: [None, None],
    };
    for index in 0..r.count(2)? {
        event.annotations[index] = Some(bexos_trace::TraceAnnotation {
            key: r.text(32)?.to_string(),
            value: r.word()? as i64,
        });
    }
    Ok(event)
}

fn encode_producer(w: &mut Encoder, producer: &TraceProducer) {
    w.word(producer.id);
    w.word(producer.pid);
    w.word(producer.main_tid);
    w.text(&producer.process_name);
    w.word(producer.categories as u64);
}

fn decode_producer(r: &mut Decoder<'_>) -> Result<TraceProducer, Error> {
    Ok(TraceProducer {
        id: r.word()?,
        pid: r.word()?,
        main_tid: r.word()?,
        process_name: r.text(64)?.to_string(),
        categories: r.word()? as u32,
    })
}

const fn buffer_mode_to_wire(mode: BufferMode) -> u64 {
    match mode {
        BufferMode::OneshotStopOnFull => 1,
        BufferMode::CircularRing => 2,
    }
}

fn buffer_mode_from_wire(value: u64) -> Result<BufferMode, Error> {
    match value {
        1 => Ok(BufferMode::OneshotStopOnFull),
        2 => Ok(BufferMode::CircularRing),
        _ => Err(Error::InvalidData),
    }
}

const fn trace_state_to_wire(state: TraceState) -> u64 {
    match state {
        TraceState::Idle => 1,
        TraceState::Recording => 2,
        TraceState::Stopped => 3,
    }
}

fn trace_state_from_wire(value: u64) -> Result<TraceState, Error> {
    match value {
        1 => Ok(TraceState::Idle),
        2 => Ok(TraceState::Recording),
        3 => Ok(TraceState::Stopped),
        _ => Err(Error::InvalidData),
    }
}

const fn event_kind_to_wire(kind: TraceEventKind) -> u64 {
    match kind {
        TraceEventKind::Metadata => 1,
        TraceEventKind::SliceBegin => 2,
        TraceEventKind::SliceEnd => 3,
        TraceEventKind::Instant => 4,
        TraceEventKind::FlowBegin => 5,
        TraceEventKind::FlowStep => 6,
        TraceEventKind::FlowEnd => 7,
        TraceEventKind::Counter => 8,
        TraceEventKind::DroppedEvents => 9,
    }
}

fn event_kind_from_wire(value: u64) -> Result<TraceEventKind, Error> {
    match value {
        1 => Ok(TraceEventKind::Metadata),
        2 => Ok(TraceEventKind::SliceBegin),
        3 => Ok(TraceEventKind::SliceEnd),
        4 => Ok(TraceEventKind::Instant),
        5 => Ok(TraceEventKind::FlowBegin),
        6 => Ok(TraceEventKind::FlowStep),
        7 => Ok(TraceEventKind::FlowEnd),
        8 => Ok(TraceEventKind::Counter),
        9 => Ok(TraceEventKind::DroppedEvents),
        _ => Err(Error::InvalidData),
    }
}
