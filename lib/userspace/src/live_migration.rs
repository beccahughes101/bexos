//! Cooperative service migration over a dedicated control endpoint.
mod changes;
use crate::{Channel, Memory, syscall, yield_now};
use alloc::{sync::Arc, vec::Vec};
use bexos_migration::{
    Error, Phase, Session, Timeouts,
    dirty::DirtySet,
    records::{Receiver, Record},
};
pub use changes::RecordChanges;
use migration_fidl::*;
pub mod wire;

pub const MAX_RECORD_DATA: usize = 32704;
pub const STATE_LIMIT: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug)]
pub enum Resource {
    Handle(u64),
    Mapping {
        handle: u64,
        offset: u64,
        va: u64,
        size: u64,
        rights: u32,
    },
    Pin(u64),
}
pub trait State: Sized {
    fn empty() -> Self;
    fn migration_state_limit() -> usize {
        STATE_LIMIT
    }
    /// Stable keys, including separate bounded records for large byte arrays.
    fn keys(&self) -> Vec<u64>;
    fn encode_record(&self, key: u64) -> Result<Option<Vec<u8>>, Error>;
    fn adopt_record(&mut self, key: u64, bytes: Option<&[u8]>) -> Result<(), Error>;
    fn validate(&self) -> Result<(), Error>;
    /// Return false while bounded device work is draining. The source keeps
    /// dispatching completion events before entering the cutover window.
    fn quiescence_ready(&self) -> bool {
        true
    }
    /// Prepare expensive derived state while the original service is still live.
    fn prepare_adoption(&mut self) -> Result<(), Error> {
        Ok(())
    }
    fn finish_adoption(&mut self) -> Result<(), Error> {
        self.validate()
    }
    fn resources(&self) -> Vec<Resource>;
    fn activation_markers(&self) -> [u64; 3] {
        [0; 3]
    }
    fn activated(&mut self, generation: u64);
}

pub fn now_ms() -> u64 {
    ((syscall::ticks() as u128 * 1000) / syscall::frequency() as u128) as u64
}

struct Transfer {
    draining: bool,
    session: Session,
    keys: Vec<u64>,
    cursor: usize,
    dirty: DirtySet,
    delta_keys: Vec<u64>,
    delta_cursor: usize,
    sequence: u64,
}
pub struct Source {
    channel: Option<Channel>,
    pending: Option<crate::Message>,
    transfer: Option<Transfer>,
    self_target: Option<Channel>,
}
impl Source {
    pub fn new(channel: Option<Channel>) -> Self {
        Self {
            channel,
            pending: None,
            transfer: None,
            self_target: None,
        }
    }
    pub fn set_self_target(&mut self, target: Channel) {
        self.self_target = Some(target);
    }
    pub fn changed(&mut self, key: u64) {
        if let Some(t) = &mut self.transfer {
            t.dirty.mark(key);
        }
    }
    pub fn changed_keys(&mut self, keys: impl IntoIterator<Item = u64>) {
        for key in keys {
            self.changed(key);
        }
    }
    pub fn quiescing(&self) -> bool {
        self.transfer
            .as_ref()
            .is_some_and(|t| t.session.phase() == Phase::Quiesced)
    }
    pub fn active(&self) -> bool {
        self.transfer.is_some()
    }
    pub fn draining(&self) -> bool {
        self.transfer.as_ref().is_some_and(|t| t.draining)
    }
    pub fn channel(&self) -> Option<Channel> {
        self.channel
    }

    /// Retain a request while a service prepares its checkpoint at a safe point.
    /// Polling an idle migration endpoint must not require encoding live state.
    pub fn has_pending(&mut self) -> Result<bool, Error> {
        if self.pending.is_none() {
            let Some(channel) = self.channel else {
                return Ok(false);
            };
            match channel.try_recv() {
                Ok(message) => self.pending = Some(message),
                Err(kernel_fidl::Status::ErrTimedOut) => return Ok(false),
                Err(_) => return Err(Error::BadState),
            }
        }
        Ok(true)
    }

    pub fn poll<S: State>(&mut self, state: &S) -> Result<(), Error> {
        self.poll_with_read(state, true)
    }
    /// Process only the request retained by `has_pending`, while still checking
    /// transfer deadlines. A newly arriving request waits for state preparation.
    pub fn poll_pending<S: State>(&mut self, state: &S) -> Result<(), Error> {
        self.poll_with_read(state, false)
    }
    fn poll_with_read<S: State>(&mut self, state: &S, read: bool) -> Result<(), Error> {
        let result = self.poll_inner(state, read);
        if result.is_err() {
            let _ = crate::migration::abort();
            self.transfer = None;
            if let Some(c) = self.self_target.take() {
                let _ = Memory::close(c.0);
            }
        }
        result
    }
    fn poll_inner<S: State>(&mut self, state: &S, read: bool) -> Result<(), Error> {
        let Some(channel) = self.channel else {
            return Ok(());
        };
        if self
            .transfer
            .as_mut()
            .is_some_and(|t| t.session.poll(now_ms()).is_err())
        {
            let _ = crate::migration::abort();
            self.transfer = None;
            if let Some(c) = self.self_target.take() {
                let _ = Memory::close(c.0);
            }
        }
        if self.pending.is_none() && (!read || !self.has_pending()?) {
            return Ok(());
        }
        let message = self.pending.take().ok_or(Error::BadState)?;
        if message.bytes.len() < 8 {
            for h in message.handles {
                let _ = Memory::close(h);
            }
            return Err(Error::InvalidData);
        }
        let ordinal = u64::from_le_bytes(message.bytes[..8].try_into().unwrap());
        let bytes = &message.bytes[8..];
        if ordinal != 1 && !message.handles.is_empty() {
            for h in message.handles {
                let _ = Memory::close(h);
            }
            return Err(Error::InvalidData);
        }
        crate::log(&alloc::format!(
            "migration: source request ordinal={ordinal}\n"
        ));
        match ordinal {
            1 => {
                let handles: Vec<_> = message
                    .handles
                    .iter()
                    .map(|h| HandleRef { raw: *h })
                    .collect();
                let q = match MigratablePrepareRequest::decode(bytes, &handles) {
                    Ok(q) if handles.len() == 1 => q,
                    _ => {
                        for h in &message.handles {
                            let _ = Memory::close(*h);
                        }
                        return Err(Error::InvalidData);
                    }
                };
                let result = (|| {
                    if q.version != bexos_migration::VERSION || self.transfer.is_some() {
                        return Err(Error::BadState);
                    }
                    let mut keys = state.keys();
                    keys.sort_unstable();
                    if keys.len() > 16384 || keys.windows(2).any(|k| k[0] == k[1]) {
                        return Err(Error::Capacity);
                    }
                    let mut session = Session::new(
                        q.generation,
                        now_ms(),
                        Timeouts {
                            preparation_ms: q.preparation_timeout_ms as u64,
                            cutover_ms: q.cutover_timeout_ms as u64,
                        },
                    )?;
                    session.begin_bulk(now_ms())?;
                    self.self_target = Some(Channel(q.candidate.raw));
                    crate::log(&alloc::format!(
                        "migration: source prepared generation={} keys={}\n",
                        q.generation,
                        keys.len()
                    ));
                    self.transfer = Some(Transfer {
                        draining: false,
                        session,
                        keys,
                        cursor: 0,
                        dirty: DirtySet::new(16384),
                        delta_keys: Vec::new(),
                        delta_cursor: 0,
                        sequence: 0,
                    });
                    Ok(())
                })();
                if result.is_err() {
                    let _ = Memory::close(q.candidate.raw);
                }
                let reply_result = reply(
                    channel,
                    q.generation,
                    ordinal,
                    &MigratablePrepareResponse {
                        status: status(&result),
                        base_sequence: 0,
                    },
                );
                crate::log(&alloc::format!(
                    "migration: source prepare reply result={reply_result:?}\n"
                ));
                reply_result?;
            }
            2 => {
                let _ = MigratableNextBulkRequest::decode(bytes, &[])
                    .map_err(|_| Error::InvalidData)?;
                let result = self.next_bulk(state);
                let (st, done, record) = match result {
                    Ok(Some(record)) => (0, false, record),
                    Ok(None) => (0, true, Vec::new()),
                    Err(_) => (-8, false, Vec::new()),
                };
                reply(
                    channel,
                    self.transfer.as_ref().map_or(0, |t| t.session.generation()),
                    ordinal,
                    &MigratableNextBulkResponse {
                        status: st,
                        done,
                        record: &record,
                    },
                )?;
            }
            3 => {
                let _ = MigratableNextDeltaRequest::decode(bytes, &[])
                    .map_err(|_| Error::InvalidData)?;
                let result = self.next_delta(state);
                let (st, caught_up, record) = match result {
                    Ok(Some(record)) => (0, false, record),
                    Ok(None) => (0, true, Vec::new()),
                    Err(_) => (-8, false, Vec::new()),
                };
                reply(
                    channel,
                    self.transfer.as_ref().map_or(0, |t| t.session.generation()),
                    ordinal,
                    &MigratableNextDeltaResponse {
                        status: st,
                        caught_up,
                        sequence: self.transfer.as_ref().map_or(0, |t| t.sequence),
                        record: &record,
                    },
                )?;
            }
            4 => {
                let _ =
                    MigratableQuiesceRequest::decode(bytes, &[]).map_err(|_| Error::InvalidData)?;
                let result = self.prepare_quiescence(state);
                let sequence = self.transfer.as_ref().map_or(0, |t| t.sequence);
                let st = match result {
                    Ok(true) => 0,
                    Ok(false) => 1,
                    Err(error) => {
                        crate::log(&alloc::format!(
                            "migration: source quiescence failed {error:?}\n"
                        ));
                        -8
                    }
                };
                reply(
                    channel,
                    self.transfer.as_ref().map_or(0, |t| t.session.generation()),
                    ordinal,
                    &MigratableQuiesceResponse {
                        status: st,
                        final_sequence: sequence,
                    },
                )?;
                if matches!(result, Ok(true)) {
                    if let Some(target) = self.self_target.take() {
                        send_request(
                            target,
                            5,
                            &StateReceiverValidateRequest {
                                final_sequence: sequence,
                            },
                        )?;
                        send_request(target, 6, &StateReceiverActivateRequest {})?;
                        let _ = Memory::close(target.0);
                    }
                    // This returns to EL0 only if the kernel aborts/resumes us.
                    let _ = crate::migration::quiesce(sequence);
                    self.transfer = None;
                } else if result.is_err() {
                    return Err(Error::BadState);
                }
            }
            6 => {
                let _ =
                    MigratableAbortRequest::decode(bytes, &[]).map_err(|_| Error::InvalidData)?;
                let _ = crate::migration::abort();
                self.transfer = None;
                if let Some(c) = self.self_target.take() {
                    let _ = Memory::close(c.0);
                }
                reply(
                    channel,
                    self.transfer.as_ref().map_or(0, |t| t.session.generation()),
                    ordinal,
                    &MigratableAbortResponse { status: 0 },
                )?;
            }
            7 => {
                let _ = MigratableGetStatusRequest::decode(bytes, &[])
                    .map_err(|_| Error::InvalidData)?;
                let phase = self.transfer.as_ref().map_or(MigrationPhase::Aborted, |t| {
                    match t.session.phase() {
                        Phase::Bulk => MigrationPhase::Bulk,
                        Phase::CatchUp => MigrationPhase::CatchUp,
                        Phase::Quiesced => MigrationPhase::Quiesced,
                        _ => MigrationPhase::Aborted,
                    }
                });
                reply(
                    channel,
                    self.transfer.as_ref().map_or(0, |t| t.session.generation()),
                    ordinal,
                    &MigratableGetStatusResponse {
                        status: 0,
                        phase,
                        generation: self.transfer.as_ref().map_or(0, |t| t.session.generation()),
                        sequence: self.transfer.as_ref().map_or(0, |t| t.sequence),
                    },
                )?;
            }
            _ => return Err(Error::BadState),
        }
        Ok(())
    }
    fn next_bulk<S: State>(&mut self, state: &S) -> Result<Option<Vec<u8>>, Error> {
        let t = self.transfer.as_mut().ok_or(Error::BadState)?;
        if t.session.phase() != Phase::Bulk {
            return Err(Error::BadState);
        }
        if t.cursor == t.keys.len() {
            t.session.bulk_complete(now_ms())?;
            crate::migration::complete_bulk().map_err(|_| Error::BadState)?;
            return Ok(None);
        }
        let key = t.keys[t.cursor];
        crate::log(&alloc::format!(
            "migration: source bulk record key={} index={} total={}\n",
            key,
            t.cursor,
            t.keys.len()
        ));
        let record = encoded_record(state, key, 0, t.session.generation())?;
        t.dirty.copied(key);
        t.cursor += 1;
        Ok(Some(record))
    }
    fn next_delta<S: State>(&mut self, state: &S) -> Result<Option<Vec<u8>>, Error> {
        let t = self.transfer.as_mut().ok_or(Error::BadState)?;
        if !matches!(t.session.phase(), Phase::CatchUp | Phase::Quiesced) {
            return Err(Error::BadState);
        }
        if t.delta_cursor == t.delta_keys.len() {
            t.delta_keys = t.dirty.snapshot_keys()?;
            t.delta_cursor = 0;
        }
        if t.sequence % 128 == 0 {
            crate::log(&alloc::format!(
                "migration: live catch-up sequence={} remaining={}\n",
                t.sequence,
                t.dirty.len()
            ));
        }
        let Some(&key) = t.delta_keys.get(t.delta_cursor) else {
            return Ok(None);
        };
        let sequence = t.sequence.checked_add(1).ok_or(Error::Sequence)?;
        let record = encoded_record(state, key, sequence, t.session.generation())?;
        t.sequence = sequence;
        t.dirty.copied(key);
        t.delta_cursor += 1;
        Ok(Some(record))
    }
    fn prepare_quiescence<S: State>(&mut self, state: &S) -> Result<bool, Error> {
        let target = self.self_target.ok_or(Error::BadState)?;
        self.transfer.as_mut().ok_or(Error::BadState)?.draining = true;
        if !state.quiescence_ready() {
            return Ok(false);
        }
        let transfer = self.transfer.as_ref().ok_or(Error::BadState)?;
        transfer.dirty.next()?;
        // A record can occupy almost 32 KiB. Keep the frozen tail to one
        // record so bulk-sized data does not consume the cutover window in
        // repeated source/candidate scheduling and copy round trips.
        const CUTOVER_TAIL_RECORDS: usize = 1;
        if transfer.session.phase() == Phase::CatchUp && transfer.dirty.len() > CUTOVER_TAIL_RECORDS
        {
            let generation = transfer.session.generation();
            // The coordinator is waiting on this source response, so it cannot
            // consume acknowledgements from the candidate's shared endpoint.
            // Copy a bounded live batch directly, avoiding three coordinator
            // round trips per record. Return to normal dispatch between batches.
            for _ in 0..8 {
                if self.transfer.as_ref().ok_or(Error::BadState)?.dirty.len()
                    <= CUTOVER_TAIL_RECORDS
                {
                    break;
                }
                let record = self.next_delta(state)?.ok_or(Error::BadState)?;
                send_delta(target, generation, &record)?;
            }
            let remaining = self.transfer.as_ref().ok_or(Error::BadState)?.dirty.len();
            crate::log(&alloc::format!(
                "migration: live catch-up batch remaining={remaining}\n"
            ));
            if remaining > CUTOVER_TAIL_RECORDS {
                return Ok(false);
            }
        }
        if let Err(error) = state.validate() {
            crate::log(&alloc::format!(
                "migration: source state validation failed {error:?}\n"
            ));
            return Err(error);
        }
        crate::log("migration: preserving source resources before cutover\n");
        for resource in state.resources() {
            let r = match resource {
                Resource::Handle(h) => crate::migration::preserve_handle(h),
                Resource::Mapping {
                    handle,
                    offset,
                    va,
                    size,
                    rights,
                } => crate::migration::preserve_mapping(handle, offset, va, size, rights),
                Resource::Pin(pin) => crate::migration::preserve_pin(pin),
            };
            if let Err(status) = r {
                crate::log(&alloc::format!(
                    "migration: resource preserve failed {resource:?} {status:?}\n"
                ));
                return Err(Error::BadState);
            }
        }
        if let Some(channel) = self.channel {
            crate::migration::preserve_handle(channel.0).map_err(|_| Error::BadState)?;
        }
        crate::log("migration: source resources preserved\n");
        let t = self.transfer.as_mut().ok_or(Error::BadState)?;
        if t.session.phase() == Phase::CatchUp {
            crate::log(&alloc::format!(
                "migration: final dirty records={}\n",
                t.dirty.len()
            ));
            t.dirty.next()?;
        }
        // Dispatch is stopped while this handler runs. Drain the bounded final
        // coalesced set directly to the candidate so the cutover does not need
        // extra coordinator/normal-service round trips.
        while let Some(key) = t.dirty.next()? {
            let sequence = t.sequence.checked_add(1).ok_or(Error::Sequence)?;
            let record = encoded_record(state, key, sequence, t.session.generation())?;
            send_delta(target, t.session.generation(), &record)?;
            t.sequence = sequence;
            t.dirty.copied(key);
        }
        t.session.poll(now_ms())?;
        Ok(true)
    }
}

fn send_delta(target: Channel, generation: u64, record: &[u8]) -> Result<(), Error> {
    send_request(target, 4, &StateReceiverAdoptDeltaRequest { record })?;
    let message = target.recv().map_err(|_| Error::BadState)?;
    let has_handles = !message.handles.is_empty();
    for handle in message.handles {
        let _ = Memory::close(handle);
    }
    wire::decode_delta_ack(&message.bytes, has_handles, generation)
}

fn encoded_record<S: State>(
    state: &S,
    key: u64,
    sequence: u64,
    generation: u64,
) -> Result<Vec<u8>, Error> {
    let data = state.encode_record(key)?;
    if data.as_ref().is_some_and(|d| d.len() > MAX_RECORD_DATA) {
        return Err(Error::Capacity);
    }
    Ok(Record {
        key,
        sequence,
        data: data.map(Arc::from),
    }
    .encode(generation))
}

/// A candidate has only its staging channel and private memory until commit.
pub fn receive<S: State>(channel: Channel, expected_generation: u64) -> Result<S, Error> {
    receive_with_state(channel, expected_generation, S::empty())
}

/// Seed a receiver with trusted replacement configuration before adopting any
/// untrusted state records. This avoids process-global candidate factories.
pub fn receive_with_state<S: State>(
    channel: Channel,
    expected_generation: u64,
    mut state: S,
) -> Result<S, Error> {
    let mut receiver: Option<Receiver> = None;
    let mut final_sequence = None;
    let mut adopted_records = 0usize;
    loop {
        let message = channel.recv().map_err(|_| Error::TimedOut)?;
        for handle in &message.handles {
            let _ = Memory::close(*handle);
        }
        if message.bytes.len() < 8 || !message.handles.is_empty() {
            return Err(Error::InvalidData);
        }
        let ordinal = u64::from_le_bytes(message.bytes[..8].try_into().unwrap());
        let bytes = &message.bytes[8..];
        match ordinal {
            1 => {
                let q = StateReceiverInitializeRequest::decode(bytes, &[])
                    .map_err(|_| Error::InvalidData)?;
                if q.version != bexos_migration::VERSION
                    || q.generation != expected_generation
                    || receiver.is_some()
                {
                    return Err(Error::InvalidData);
                }
                let state_limit = S::migration_state_limit();
                crate::log(&alloc::format!(
                    "migration: target initialized logical_state_limit={}\n",
                    state_limit
                ));
                receiver = Some(Receiver::new_tracking(q.base_sequence, state_limit));
                reply(
                    channel,
                    expected_generation,
                    ordinal,
                    &StateReceiverInitializeResponse { status: 0 },
                )?;
            }
            2 | 4 => {
                let data = if ordinal == 2 {
                    StateReceiverAdoptBulkRequest::decode(bytes, &[])
                        .map_err(|_| Error::InvalidData)?
                        .record
                } else {
                    StateReceiverAdoptDeltaRequest::decode(bytes, &[])
                        .map_err(|_| Error::InvalidData)?
                        .record
                };
                let record = Record::decode(data, expected_generation, MAX_RECORD_DATA)?;
                if adopted_records % 128 == 0 {
                    crate::log(&alloc::format!(
                        "migration: target adopting record index={} phase={} key={} bytes={}\n",
                        adopted_records,
                        if ordinal == 2 { "bulk" } else { "delta" },
                        record.key,
                        record.data.as_ref().map_or(0, |data| data.len()),
                    ));
                }
                let r = receiver.as_mut().ok_or(Error::BadState)?;
                if ordinal == 2 {
                    r.bulk(record.clone())?;
                } else {
                    r.delta(record.clone())?;
                }
                if let Err(error) = state.adopt_record(record.key, record.data.as_deref()) {
                    crate::log(&alloc::format!(
                        "migration: target record adoption failed key={} error={error:?}\n",
                        record.key
                    ));
                    return Err(error);
                }
                adopted_records += 1;
                if ordinal == 2 {
                    reply(
                        channel,
                        expected_generation,
                        ordinal,
                        &StateReceiverAdoptBulkResponse { status: 0 },
                    )?;
                } else {
                    reply(
                        channel,
                        expected_generation,
                        ordinal,
                        &StateReceiverAdoptDeltaResponse { status: 0 },
                    )?;
                }
            }
            3 => {
                let _ = StateReceiverCompleteBulkRequest::decode(bytes, &[])
                    .map_err(|_| Error::InvalidData)?;
                receiver.as_mut().ok_or(Error::BadState)?.finish_bulk()?;
                state.prepare_adoption()?;
                crate::log(&alloc::format!(
                    "migration: target bulk adoption complete records={}\n",
                    adopted_records
                ));
                reply(
                    channel,
                    expected_generation,
                    ordinal,
                    &StateReceiverCompleteBulkResponse { status: 0 },
                )?;
            }
            5 => {
                let q = StateReceiverValidateRequest::decode(bytes, &[])
                    .map_err(|_| Error::InvalidData)?;
                let record_keys = receiver
                    .take()
                    .ok_or(Error::BadState)?
                    .finish_keys(q.final_sequence)?;
                if let Err(error) = state.finish_adoption() {
                    crate::log(&alloc::format!(
                        "migration: target final validation failed error={error:?}\n"
                    ));
                    return Err(error);
                }
                let mut expected = state.keys();
                expected.sort_unstable();
                if expected != record_keys {
                    return Err(Error::InvalidData);
                }
                final_sequence = Some(q.final_sequence);
                reply(
                    channel,
                    expected_generation,
                    ordinal,
                    &StateReceiverValidateResponse { status: 0 },
                )?;
            }
            6 => {
                crate::log("migration: target received activation request\n");
                let _ = StateReceiverActivateRequest::decode(bytes, &[])
                    .map_err(|_| Error::InvalidData)?;
                let sequence = final_sequence.ok_or(Error::BadState)?;
                let deadline = now_ms() + 150;
                loop {
                    match crate::migration::ready(sequence) {
                        Ok(()) => break,
                        Err(kernel_fidl::Status::ErrInvalidArgs) if now_ms() < deadline => {
                            yield_now()
                        }
                        Err(_) => return Err(Error::BadState),
                    }
                }
                crate::migration::commit().map_err(|error| {
                    crate::log(&alloc::format!(
                        "migration: target commit rejected {error:?}\n"
                    ));
                    Error::BadState
                })?;
                crate::log("migration: target commit returned\n");
                state.activated(expected_generation);
                let markers = state.activation_markers();
                let result = reply(
                    channel,
                    expected_generation,
                    ordinal,
                    &StateReceiverActivateResponse {
                        status: 0,
                        marker0: markers[0],
                        marker1: markers[1],
                        marker2: markers[2],
                    },
                );
                crate::log(&alloc::format!(
                    "migration: target activation reply result={result:?}\n"
                ));
                // A successful kernel commit retires the source process before
                // a self-migrating service can read this acknowledgement. The
                // target is already authoritative at that point, so a closed
                // peer must not terminate the committed replacement.
                return Ok(state);
            }
            7 => {
                let _ = crate::migration::abort();
                return Err(Error::BadState);
            }
            _ => return Err(Error::InvalidData),
        }
        yield_now();
    }
}

fn status(result: &Result<(), Error>) -> i32 {
    if result.is_ok() { 0 } else { -8 }
}
fn send_request<Q: FidlEncode>(channel: Channel, ordinal: u64, q: &Q) -> Result<(), Error> {
    let bytes = wire::encode_request(ordinal, q)?;
    channel.send(&bytes, &[]).map_err(|_| Error::BadState)
}
fn reply<Q: FidlEncode>(
    channel: Channel,
    generation: u64,
    ordinal: u64,
    q: &Q,
) -> Result<(), Error> {
    // Cutover acknowledgements are fixed-size. Avoid allocating and clearing
    // a maximum-size record buffer while the source is quiesced.
    let mut inline = [0u8; 128];
    inline[..8].copy_from_slice(&generation.to_le_bytes());
    inline[8..16].copy_from_slice(&ordinal.to_le_bytes());
    match q.encode(&mut inline[16..], &mut []) {
        Ok(encoded) => {
            return channel
                .send(&inline[..16 + encoded.bytes], &[])
                .map_err(|_| Error::BadState);
        }
        Err(FidlWireError::BufferTooSmall) => {}
        Err(_) => return Err(Error::InvalidData),
    }
    let mut bytes = alloc::vec![0; 65500];
    bytes[..8].copy_from_slice(&generation.to_le_bytes());
    bytes[8..16].copy_from_slice(&ordinal.to_le_bytes());
    let e = q
        .encode(&mut bytes[16..], &mut [])
        .map_err(|_| Error::InvalidData)?;
    channel
        .send(&bytes[..16 + e.bytes], &[])
        .map_err(|_| Error::BadState)
}
