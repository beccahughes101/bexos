//! Preference RPC helpers and opt-in live configuration receiver.
use crate::{Channel, Memory, Startup};
use alloc::{collections::BTreeSet, string::String, sync::Arc, vec, vec::Vec};
use bexos_component_config::{
    ConfigTable, MAX_CONFIG_SNAPSHOT_LEN,
    schema::{Error, Schema, Value},
    transaction::Receiver,
};
pub use preferences_fidl as wire;
use wire::{FidlDecode, FidlEncode, HandleRef, Status};

const PREFERENCE_RPC_TIMEOUT_SECONDS: u64 = 900;

pub fn status(error: Error) -> Status {
    match error {
        Error::UnknownField => Status::NotFound,
        Error::AccessDenied => Status::AccessDenied,
        Error::Conflict => Status::Conflict,
        Error::Busy => Status::Busy,
        Error::Storage => Status::Storage,
        Error::Rejected => Status::Rejected,
        Error::Timeout => Status::Timeout,
        _ => Status::InvalidArgs,
    }
}
pub fn value(v: &wire::ConfigValue<'_>) -> Value {
    match v {
        wire::ConfigValue::BoolValue(v) => Value::Bool(v.value),
        wire::ConfigValue::Uint32Value(v) => Value::Uint32(v.value),
        wire::ConfigValue::Uint64Value(v) => Value::Uint64(v.value),
        wire::ConfigValue::StringValue(v) => Value::String(v.value.into()),
        wire::ConfigValue::BytesValue(v) => Value::Bytes(v.value.to_vec()),
    }
}
pub fn call<Q: FidlEncode>(channel: Channel, ordinal: u64, q: &Q) -> Result<crate::Message, Error> {
    let mut bytes = vec![0; 256 * 1024];
    let mut hs = [HandleRef { raw: 0 }; 16];
    let e = q
        .encode(&mut bytes[8..], &mut hs)
        .map_err(|_| Error::Malformed)?;
    bytes[..8].copy_from_slice(&ordinal.to_le_bytes());
    let handles = hs[..e.handles].iter().map(|h| h.raw).collect::<Vec<_>>();
    if channel.send(&bytes[..8 + e.bytes], &handles).is_err() {
        for handle in handles {
            let _ = Memory::close(handle);
        }
        return Err(Error::Storage);
    }
    channel
        .recv_with_timeout(PREFERENCE_RPC_TIMEOUT_SECONDS)
        .map_err(|error| {
            crate::log(&alloc::format!(
                "preferences: RPC ordinal={ordinal} receive failed {error:?}\n"
            ));
            Error::Storage
        })
}
pub fn send<Q: FidlEncode>(channel: Channel, ordinal: u64, q: &Q) -> Result<(), Error> {
    let mut bytes = vec![0; 256 * 1024];
    let mut hs = [HandleRef { raw: 0 }; 16];
    let e = q
        .encode(&mut bytes[8..], &mut hs)
        .map_err(|_| Error::Malformed)?;
    bytes[..8].copy_from_slice(&ordinal.to_le_bytes());
    channel
        .send(
            &bytes[..8 + e.bytes],
            &hs[..e.handles].iter().map(|h| h.raw).collect::<Vec<_>>(),
        )
        .map_err(|_| Error::Storage)
}
pub fn reply<Q: FidlEncode>(channel: Channel, q: &Q) -> Result<(), Error> {
    let mut bytes = vec![0; 256 * 1024];
    let mut hs = [HandleRef { raw: 0 }; 16];
    let e = q
        .encode(&mut bytes, &mut hs)
        .map_err(|_| Error::Malformed)?;
    let result = channel
        .send(
            &bytes[..e.bytes],
            &hs[..e.handles].iter().map(|h| h.raw).collect::<Vec<_>>(),
        )
        .map_err(|_| Error::Storage);
    if result.is_err() {
        for handle in &hs[..e.handles] {
            let _ = Memory::close(handle.raw);
        }
    }
    result
}
pub fn refs(message: &crate::Message) -> Vec<HandleRef> {
    message
        .handles
        .iter()
        .map(|h| HandleRef { raw: *h })
        .collect()
}
pub fn read_vmo(handle: u64, len: u64) -> Result<Vec<u8>, Error> {
    let result = (|| {
        if len == 0 || len > MAX_CONFIG_SNAPSHOT_LEN as u64 {
            return Err(Error::Bounds);
        }
        let rounded = len.checked_add(4095).ok_or(Error::Overflow)? & !4095;
        let va = Memory::map(handle, rounded, 2).map_err(|_| Error::Storage)?;
        let bytes = unsafe { core::slice::from_raw_parts(va as *const u8, len as usize) }.to_vec();
        let _ = Memory::unmap(va, rounded);
        Ok(bytes)
    })();
    let _ = Memory::close(handle);
    result
}
pub fn read_only_vmo(bytes: &[u8]) -> Result<u64, Error> {
    let raw = Memory::from_bytes(bytes).map_err(|_| Error::Storage)?;
    let result = Memory::duplicate(raw, 1 | 2 | 16 | 32).map_err(|_| Error::Storage);
    let _ = Memory::close(raw);
    result
}
pub fn decode_policy(schema: &Schema, bytes: &[u8]) -> Result<BTreeSet<String>, Error> {
    let mut locks = BTreeSet::new();
    let mut fp = None;
    for f in bexos_component_config::wire::fields(bytes) {
        let (n, k, b) = f?;
        match (n, k) {
            (1, 0) => fp = Some(bexos_component_config::wire::varint(b)?.0),
            (2, 2) => {
                let name = core::str::from_utf8(b).map_err(|_| Error::Malformed)?;
                if !locks.insert(name.into()) {
                    return Err(Error::Malformed);
                }
            }
            _ => return Err(Error::Malformed),
        }
    }
    if !bytes.is_empty() && fp != Some(schema.fingerprint()) {
        return Err(Error::Fingerprint);
    }
    schema.validate_locks(&locks)?;
    Ok(locks)
}

pub trait DecodeConfig: Sized {
    fn from_config(table: ConfigTable<'_>) -> Result<Self, crate::config::ConfigError>;
}
pub struct LiveConfig<T> {
    endpoint: Channel,
    state: Receiver<T>,
    pending_transaction: Option<u64>,
    committed_transaction: Option<u64>,
}
impl<T: DecodeConfig> LiveConfig<T> {
    pub fn from_startup(startup: &Startup) -> Result<Self, Error> {
        let endpoint = startup.config_endpoint.ok_or(Error::UnknownField)?;
        let result = (|| {
            let bytes = read_vmo(
                startup.config.ok_or(Error::MissingField)?,
                startup.config_len,
            )?;
            let table = ConfigTable::parse(&bytes).map_err(|_| Error::Malformed)?;
            T::from_config(table).map_err(|_| Error::Malformed)?;
            let start = crate::syscall::ticks();
            loop {
                let response = call(endpoint, 1, &wire::ConfigObserverRegisterRequest {})?;
                let handles = refs(&response);
                let r = wire::ConfigObserverRegisterResponse::decode(&response.bytes, &handles)
                    .map_err(|_| Error::Malformed)?;
                if r.status == Status::Busy
                    && crate::syscall::ticks().saturating_sub(start)
                        < crate::syscall::frequency() * 300
                {
                    crate::yield_now();
                    continue;
                }
                if r.status != Status::Ok {
                    return Err(Error::Busy);
                }
                let bytes = read_vmo(
                    r.config.first().ok_or(Error::MissingField)?.raw,
                    r.config_len,
                )?;
                let table = ConfigTable::parse(&bytes).map_err(|_| Error::Malformed)?;
                let generation = table.generation();
                let value = T::from_config(table).map_err(|_| Error::Malformed)?;
                return Ok(Self {
                    endpoint,
                    state: Receiver::new(value, generation),
                    pending_transaction: None,
                    committed_transaction: None,
                });
            }
        })();
        if result.is_err() {
            let _ = Memory::close(endpoint.0);
        }
        result
    }
    pub fn snapshot(&self) -> Arc<T> {
        self.state.snapshot()
    }
    pub fn generation(&self) -> u64 {
        self.state.generation()
    }
    pub fn endpoint(&self) -> Channel {
        self.endpoint
    }
    /// Call from the application event loop. Callbacks execute on that thread.
    pub fn poll(
        &mut self,
        validate: impl FnOnce(&T) -> bool,
        changed: impl FnOnce(&T),
    ) -> Result<bool, Error> {
        let message = match self.endpoint.try_recv() {
            Ok(m) => m,
            Err(kernel_fidl::Status::ErrTimedOut) => return Ok(false),
            Err(_) => return Err(Error::Storage),
        };
        if message.bytes.len() < 8 {
            for handle in message.handles {
                let _ = Memory::close(handle);
            }
            return Err(Error::Malformed);
        }
        let ordinal = message
            .bytes
            .get(..8)
            .map(|b| u64::from_le_bytes(b.try_into().unwrap()))
            .ok_or(Error::Malformed)?;
        if message.handles.len() != usize::from(ordinal == 2) {
            for handle in message.handles {
                let _ = Memory::close(handle);
            }
            return Err(Error::Malformed);
        }
        let hs = refs(&message);
        let body = &message.bytes[8..];
        match ordinal {
            2 => {
                let q = wire::ConfigObserverPrepareRequest::decode(body, &hs).map_err(|_| {
                    let _ = Memory::close(message.handles[0]);
                    Error::Malformed
                })?;
                let result = read_vmo(q.config.raw, q.config_len)
                    .and_then(|b| {
                        let table = ConfigTable::parse(&b).map_err(|_| Error::Malformed)?;
                        if table.generation() != q.generation {
                            return Err(Error::Conflict);
                        }
                        T::from_config(table).map_err(|_| Error::Malformed)
                    })
                    .and_then(|v| {
                        if self
                            .pending_transaction
                            .is_some_and(|id| id != q.transaction_id)
                        {
                            return Err(Error::Busy);
                        }
                        self.state.prepare(q.generation, v, validate)?;
                        self.pending_transaction = Some(q.transaction_id);
                        Ok(())
                    });
                reply(
                    self.endpoint,
                    &wire::ConfigObserverPrepareResponse {
                        transaction_id: q.transaction_id,
                        status: result.map_or_else(status, |_| Status::Ok),
                        generation: q.generation,
                    },
                )?;
            }
            3 => {
                let q = wire::ConfigObserverCommitRequest::decode(body, &hs)
                    .map_err(|_| Error::Malformed)?;
                let result = if self.committed_transaction == Some(q.transaction_id)
                    && q.generation == self.state.generation()
                {
                    Ok(())
                } else if self.pending_transaction == Some(q.transaction_id) {
                    self.state.commit(q.generation, changed).map(|()| {
                        self.pending_transaction = None;
                        self.committed_transaction = Some(q.transaction_id);
                    })
                } else {
                    Err(Error::Conflict)
                };
                reply(
                    self.endpoint,
                    &wire::ConfigObserverCommitResponse {
                        transaction_id: q.transaction_id,
                        status: result.map_or_else(status, |_| Status::Ok),
                        generation: q.generation,
                    },
                )?;
            }
            4 => {
                let q = wire::ConfigObserverAbortRequest::decode(body, &hs)
                    .map_err(|_| Error::Malformed)?;
                if self.pending_transaction == Some(q.transaction_id) {
                    self.state.abort(q.generation);
                    self.pending_transaction = None;
                }
            }
            _ => {
                for h in message.handles {
                    let _ = Memory::close(h);
                }
                return Err(Error::Malformed);
            }
        }
        Ok(true)
    }
}

impl<T> Drop for LiveConfig<T> {
    fn drop(&mut self) {
        let _ = Memory::close(self.endpoint.0);
    }
}
