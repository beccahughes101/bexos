//! Complete provider state and authenticated endpoint handoff.
use crate::{binding::Client, runtime::Runtime, service::*};
use alloc::{
    collections::{BTreeMap, BTreeSet},
    string::String,
    vec::Vec,
};
use bexos_component_config::{
    schema::{Assignments, Schema, Value},
    transaction::{Phase, PreparedSnapshot, Transaction},
};
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_userspace::{
    Channel,
    live_migration::{Resource, State},
};
fn count(r: &mut Decoder<'_>) -> Result<usize, Error> {
    let n = r.word()?;
    if n > 65536 {
        return Err(Error::InvalidData);
    }
    Ok(n as usize)
}
fn assignments(w: &mut Encoder, values: &Assignments) {
    w.word(values.len() as u64);
    for (n, v) in values {
        w.text(n);
        w.bytes(&v.encode());
    }
}
fn read_assignments(r: &mut Decoder<'_>) -> Result<Assignments, Error> {
    let mut values = Assignments::new();
    for _ in 0..count(r)? {
        let n = r.text(64)?.into();
        let v = Value::decode(r.bytes(65536)?).map_err(|_| Error::InvalidData)?;
        if values.insert(n, v).is_some() {
            return Err(Error::InvalidData);
        }
    }
    Ok(values)
}
fn strings(w: &mut Encoder, values: &BTreeSet<String>) {
    w.word(values.len() as u64);
    for s in values {
        w.text(s);
    }
}
fn read_strings(r: &mut Decoder<'_>) -> Result<BTreeSet<String>, Error> {
    let mut values = BTreeSet::new();
    for _ in 0..count(r)? {
        if !values.insert(r.text(128)?.into()) {
            return Err(Error::InvalidData);
        }
    }
    Ok(values)
}
fn words(w: &mut Encoder, values: &BTreeSet<u64>) {
    w.word(values.len() as u64);
    for v in values {
        w.word(*v);
    }
}
fn read_words(r: &mut Decoder<'_>) -> Result<BTreeSet<u64>, Error> {
    let mut values = BTreeSet::new();
    for _ in 0..count(r)? {
        if !values.insert(r.word()?) {
            return Err(Error::InvalidData);
        }
    }
    Ok(values)
}
fn mutation(w: &mut Encoder, m: &Mutation) {
    match m {
        Mutation::User {
            uid,
            package,
            fingerprint,
            values,
        } => {
            w.word(1);
            w.word(*uid);
            w.text(package);
            w.word(*fingerprint);
            assignments(w, values);
        }
        Mutation::Operator {
            package,
            values,
            locks,
        } => {
            w.word(2);
            w.text(package);
            assignments(w, values);
            strings(w, locks);
        }
    }
}
fn read_mutation(r: &mut Decoder<'_>) -> Result<Mutation, Error> {
    match r.word()? {
        1 => Ok(Mutation::User {
            uid: r.word()?,
            package: r.text(128)?.into(),
            fingerprint: r.word()?,
            values: read_assignments(r)?,
        }),
        2 => Ok(Mutation::Operator {
            package: r.text(128)?.into(),
            values: read_assignments(r)?,
            locks: read_strings(r)?,
        }),
        _ => Err(Error::InvalidData),
    }
}
impl Runtime {
    fn snapshot_record(&self, key: u64) -> Result<Option<Vec<u8>>, Error> {
        if key != 0 {
            return Err(Error::InvalidData);
        }
        let mut w = Encoder::new();
        w.word(3);
        w.word(if cfg!(bexos_arch_x86_64) { 2 } else { 1 });
        for v in [
            self.control.0,
            self.migration.map_or(0, |c| c.0),
            self.vfsd.0,
            self.usersd.0,
            self.watcher.0,
            self.timeout_ms,
            self.generation,
            self.service.sequence,
        ] {
            w.word(v);
        }
        w.word(u64::from(self.service.frozen));
        w.word(self.clients.len() as u64);
        for c in &self.clients {
            w.word(c.channel);
            w.text(&c.package);
            w.text(&c.package_key);
            w.word(c.uid);
            w.word(u64::from(c.admin));
            w.word(u64::from(c.manage));
            w.word(u64::from(c.theme));
            w.word(u64::from(c.locale));
            w.word(c.methods.len() as u64);
            for m in &c.methods {
                w.word(*m);
            }
        }
        w.word(self.service.packages.len() as u64);
        for p in self.service.packages.values() {
            w.text(&p.key);
            w.text(&p.id);
            w.bytes(&p.schema.encode());
            assignments(&mut w, &p.base);
            strings(&mut w, &p.product_locks);
        }
        w.word(self.service.operators.len() as u64);
        for (k, o) in &self.service.operators {
            w.text(k);
            w.word(o.generation);
            assignments(&mut w, &o.values);
            strings(&mut w, &o.locks);
        }
        w.word(self.service.preferences.len() as u64);
        for ((uid, id, fp), p) in &self.service.preferences {
            w.word(*uid);
            w.text(id);
            w.word(*fp);
            w.word(p.generation);
            assignments(&mut w, &p.values);
            w.word(p.revisions.len() as u64);
            for (k, v) in &p.revisions {
                w.text(k);
                w.word(*v);
            }
        }
        w.word(self.service.observers.len() as u64);
        for o in &self.service.observers {
            w.word(o.channel);
            w.text(&o.package);
            w.word(o.uid);
            w.word(u64::from(o.registered));
        }
        w.word(self.service.theme_observers.len() as u64);
        for o in &self.service.theme_observers {
            w.word(o.channel);
            w.word(o.uid);
            w.word(o.generation);
        }
        w.word(self.service.locale_observers.len() as u64);
        for o in &self.service.locale_observers {
            w.word(o.channel);
            w.word(o.uid);
            w.word(o.generation);
        }
        words(&mut w, &self.service.locked);
        w.word(u64::from(self.service.pending.is_some()));
        if let Some(p) = &self.service.pending {
            let t = &p.transaction;
            w.word(u64::from(p.delivery_failed));
            w.word(t.generation);
            w.word(t.deadline_ms);
            w.word(match t.phase {
                Phase::Preparing => 1,
                Phase::Prepared => 2,
                Phase::Committed => 3,
                Phase::Complete => 4,
                Phase::Aborted => 5,
                Phase::Resolving => 6,
            });
            words(&mut w, &t.participants);
            words(&mut w, &t.awaiting);
            mutation(&mut w, &p.mutation);
            w.word(p.reply_channel);
            w.word(p.reply_ordinal);
            w.word(p.response_generation);
            w.word(p.snapshots.len() as u64);
            for s in &p.snapshots {
                w.word(s.receiver);
                w.bytes(&s.bytes);
            }
        }
        Ok(Some(w.finish()))
    }
    fn restore_record(&mut self, key: u64, bytes: Option<&[u8]>) -> Result<(), Error> {
        if key != 0 {
            return Err(Error::InvalidData);
        }
        let mut r = Decoder::new(bytes.ok_or(Error::InvalidData)?);
        let snapshot_version = r.word()?;
        if !(1..=3).contains(&snapshot_version)
            || r.word()? != if cfg!(bexos_arch_x86_64) { 2 } else { 1 }
        {
            return Err(Error::UnsupportedVersion);
        }
        let mut next = Runtime::empty();
        next.control = Channel(r.word()?);
        next.migration = Some(Channel(r.word()?));
        next.vfsd = Channel(r.word()?);
        next.usersd = Channel(r.word()?);
        next.watcher = Channel(r.word()?);
        next.timeout_ms = r.word()?;
        next.generation = r.word()?;
        next.service.sequence = r.word()?;
        let frozen = r.word()? != 0;
        for _ in 0..count(&mut r)? {
            let channel = r.word()?;
            let package = r.text(128)?.into();
            let package_key = r.text(128)?.into();
            let uid = r.word()?;
            let admin = r.word()? != 0;
            let manage = r.word()? != 0;
            let theme = if snapshot_version >= 2 {
                r.word()? != 0
            } else {
                false
            };
            let locale = snapshot_version >= 3 && r.flag()?;
            let mut methods = Vec::new();
            for _ in 0..count(&mut r)? {
                methods.push(r.word()?);
            }
            next.clients.push(Client {
                channel,
                package,
                package_key,
                uid,
                admin,
                manage,
                theme,
                locale,
                methods,
            });
        }
        for _ in 0..count(&mut r)? {
            let key = r.text(128)?.into();
            let id = r.text(128)?.into();
            let schema = Schema::decode(r.bytes(65536)?).map_err(|_| Error::InvalidData)?;
            let base = read_assignments(&mut r)?;
            let product_locks = read_strings(&mut r)?;
            next.service
                .register(Package {
                    key,
                    id,
                    schema,
                    base,
                    product_locks,
                })
                .map_err(|_| Error::InvalidData)?;
        }
        for _ in 0..count(&mut r)? {
            let key = r.text(128)?.into();
            let generation = r.word()?;
            let values = read_assignments(&mut r)?;
            let locks = read_strings(&mut r)?;
            next.service.operators.insert(
                key,
                Operator {
                    generation,
                    values,
                    locks,
                },
            );
        }
        for _ in 0..count(&mut r)? {
            let uid = r.word()?;
            let id = r.text(128)?.into();
            let fp = r.word()?;
            let generation = r.word()?;
            let values = read_assignments(&mut r)?;
            let mut revisions = BTreeMap::new();
            for _ in 0..count(&mut r)? {
                revisions.insert(r.text(128)?.into(), r.word()?);
            }
            next.service.preferences.insert(
                (uid, id, fp),
                Preferences {
                    generation,
                    values,
                    revisions,
                },
            );
        }
        for _ in 0..count(&mut r)? {
            next.service.observers.push(Observer {
                channel: r.word()?,
                package: r.text(128)?.into(),
                uid: r.word()?,
                registered: r.word()? != 0,
            });
        }
        if snapshot_version >= 2 {
            for _ in 0..count(&mut r)? {
                next.service.theme_observers.push(ThemeObserver {
                    channel: r.word()?,
                    uid: r.word()?,
                    generation: r.word()?,
                });
            }
        }
        if snapshot_version >= 3 {
            for _ in 0..r.count(256)? {
                next.service.locale_observers.push(ThemeObserver {
                    channel: r.word()?,
                    uid: r.word()?,
                    generation: r.word()?,
                });
            }
        }
        next.service.locked = read_words(&mut r)?;
        if r.word()? != 0 {
            let delivery_failed = r.word()? != 0;
            let generation = r.word()?;
            let deadline_ms = r.word()?;
            let phase = match r.word()? {
                1 => Phase::Preparing,
                2 => Phase::Prepared,
                3 => Phase::Committed,
                4 => Phase::Complete,
                5 => Phase::Aborted,
                6 => Phase::Resolving,
                _ => return Err(Error::InvalidData),
            };
            let participants = read_words(&mut r)?;
            let awaiting = read_words(&mut r)?;
            if !awaiting.is_subset(&participants) {
                return Err(Error::InvalidData);
            }
            let mutation = read_mutation(&mut r)?;
            let reply_channel = r.word()?;
            let reply_ordinal = r.word()?;
            let response_generation = r.word()?;
            let mut snapshots = Vec::new();
            for _ in 0..count(&mut r)? {
                snapshots.push(PreparedSnapshot {
                    receiver: r.word()?,
                    bytes: r.bytes(65536)?.to_vec(),
                });
            }
            next.service.pending = Some(Pending {
                delivery_failed,
                transaction: Transaction {
                    generation,
                    deadline_ms,
                    phase,
                    participants,
                    awaiting,
                },
                mutation,
                snapshots,
                reply_channel,
                reply_ordinal,
                response_generation,
            });
        }
        next.service.frozen = frozen;
        r.finish()?;
        *self = next;
        Ok(())
    }
}
impl State for Runtime {
    fn empty() -> Self {
        Runtime::empty()
    }
    fn keys(&self) -> Vec<u64> {
        let len = self.snapshot_record(0).unwrap().unwrap().len();
        (0..1 + len.div_ceil(bexos_userspace::live_migration::MAX_RECORD_DATA) as u64).collect()
    }
    fn encode_record(&self, key: u64) -> Result<Option<Vec<u8>>, Error> {
        let bytes = self.snapshot_record(0)?.ok_or(Error::InvalidData)?;
        if key == 0 {
            return Ok(Some((bytes.len() as u64).to_le_bytes().to_vec()));
        }
        let start = usize::try_from(key - 1)
            .map_err(|_| Error::InvalidData)?
            .checked_mul(bexos_userspace::live_migration::MAX_RECORD_DATA)
            .ok_or(Error::InvalidData)?;
        Ok(bytes.get(start..).filter(|b| !b.is_empty()).map(|b| {
            b[..b
                .len()
                .min(bexos_userspace::live_migration::MAX_RECORD_DATA)]
                .to_vec()
        }))
    }
    fn adopt_record(&mut self, key: u64, bytes: Option<&[u8]>) -> Result<(), Error> {
        if key == 0 {
            let bytes = bytes.ok_or(Error::InvalidData)?;
            self.incoming_len = usize::try_from(u64::from_le_bytes(
                bytes.try_into().map_err(|_| Error::InvalidData)?,
            ))
            .map_err(|_| Error::InvalidData)?;
            if self.incoming_len == 0 || self.incoming_len > 16 * 1024 * 1024 {
                return Err(Error::InvalidData);
            }
            self.incoming.resize(
                self.incoming_len
                    .div_ceil(bexos_userspace::live_migration::MAX_RECORD_DATA),
                None,
            );
        } else {
            let i = usize::try_from(key - 1).map_err(|_| Error::InvalidData)?;
            if i >= self.incoming.len() {
                return if bytes.is_none() {
                    Ok(())
                } else {
                    Err(Error::InvalidData)
                };
            }
            if bytes.is_some_and(|b| b.len() > bexos_userspace::live_migration::MAX_RECORD_DATA) {
                return Err(Error::InvalidData);
            }
            self.incoming[i] = bytes.map(<[u8]>::to_vec);
        }
        Ok(())
    }
    fn finish_adoption(&mut self) -> Result<(), Error> {
        let mut bytes = Vec::with_capacity(self.incoming_len);
        for chunk in &self.incoming {
            bytes.extend_from_slice(chunk.as_deref().ok_or(Error::InvalidData)?);
        }
        if bytes.len() != self.incoming_len {
            return Err(Error::InvalidData);
        }
        self.restore_record(0, Some(&bytes))?;
        self.validate()
    }
    fn validate(&self) -> Result<(), Error> {
        if self.control.0 == 0
            || self.vfsd.0 == 0
            || self.usersd.0 == 0
            || !(1..=60000).contains(&self.timeout_ms)
        {
            return Err(Error::InvalidData);
        }
        for c in &self.clients {
            if c.channel == 0
                || (c.admin && (c.package != "bexos.platform.appd" || c.uid != 0))
                || (c.locale
                    && (c.package != "bexos.service.localed"
                        || c.uid != 0
                        || c.admin
                        || c.manage
                        || c.theme
                        || c.methods.iter().any(|m| !matches!(m, 1 | 2))))
            {
                return Err(Error::InvalidData);
            }
        }
        for (key, o) in &self.service.operators {
            let p = self.service.package(key).map_err(|_| Error::InvalidData)?;
            p.schema
                .validate_assignments(&o.values, false, &o.locks)
                .map_err(|_| Error::InvalidData)?;
            p.schema
                .validate_locks(&o.locks)
                .map_err(|_| Error::InvalidData)?;
        }
        for ((uid, id, fp), prefs) in &self.service.preferences {
            if self.service.locked.contains(uid) {
                return Err(Error::InvalidData);
            }
            let schema = self
                .service
                .packages
                .values()
                .find(|p| p.id == *id && p.schema.fingerprint() == *fp)
                .map(|p| &p.schema)
                .ok_or(Error::InvalidData)?;
            schema
                .validate_assignments(&prefs.values, true, &BTreeSet::new())
                .map_err(|_| Error::InvalidData)?;
        }
        for observer in &self.service.observers {
            if observer.channel == 0
                || !self.service.packages.contains_key(&observer.package)
                || self.service.locked.contains(&observer.uid)
            {
                return Err(Error::InvalidData);
            }
        }
        for observer in self
            .service
            .theme_observers
            .iter()
            .chain(&self.service.locale_observers)
        {
            if observer.channel == 0 {
                return Err(Error::InvalidData);
            }
        }
        for observer in &self.service.locale_observers {
            if self.service.locked.contains(&observer.uid) {
                return Err(Error::InvalidData);
            }
        }
        if let Some(pending) = &self.service.pending {
            for snapshot in &pending.snapshots {
                let observer = self
                    .service
                    .observers
                    .iter()
                    .find(|o| o.channel == snapshot.receiver)
                    .ok_or(Error::InvalidData)?;
                let schema = &self.service.packages[&observer.package].schema;
                schema
                    .decode_table(&snapshot.bytes)
                    .map_err(|_| Error::InvalidData)?;
            }
        }
        Ok(())
    }
    fn resources(&self) -> Vec<Resource> {
        let mut handles = BTreeSet::new();
        handles.extend([
            self.control.0,
            self.migration.map_or(0, |c| c.0),
            self.vfsd.0,
            self.usersd.0,
            self.watcher.0,
        ]);
        handles.extend(self.clients.iter().map(|c| c.channel));
        handles.extend(self.service.observers.iter().map(|o| o.channel));
        handles.extend(self.service.theme_observers.iter().map(|o| o.channel));
        handles.extend(self.service.locale_observers.iter().map(|o| o.channel));
        handles.remove(&0);
        handles.into_iter().map(Resource::Handle).collect()
    }
    fn activated(&mut self, generation: u64) {
        self.generation = generation;
    }
}
