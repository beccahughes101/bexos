//! Encrypted user records and durable operator policy, accessed only by prefsd.
use crate::service::{Mutation, Operator, Preferences, Service};
use alloc::{collections::BTreeMap, format, string::String, vec::Vec};
use bexos_component_config::{
    ConfigTable, ConfigType, encode_config_v2,
    schema::Error,
    storage::{self, Files},
};
use bexos_migration::codec::{Decoder, Encoder};
use bexos_userspace::{Channel, fs, vfs};

pub struct Directory(pub Channel);
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = bexos_userspace::Memory::close(self.0.0);
    }
}
impl Files for Directory {
    fn read(&mut self, name: &str) -> Result<Option<Vec<u8>>, Error> {
        let file = match fs::open(self.0, name, 1) {
            Ok(f) => f,
            Err(fs_fidl::FsStatus::NotFound) => return Ok(None),
            Err(e) => return Err(storage_error(&format!("read open {name}"), e)),
        };
        let result = (|| {
            let len = fs::attributes(file).map_err(|_| Error::Storage)?.size_bytes;
            if len > 65536 {
                return Err(Error::Storage);
            }
            let mut bytes = Vec::new();
            while (bytes.len() as u64) < len {
                let chunk = fs::read(file, len - bytes.len() as u64).map_err(|_| Error::Storage)?;
                if chunk.is_empty() {
                    return Err(Error::Storage);
                }
                bytes.extend(chunk);
            }
            Ok(Some(bytes))
        })();
        let _ = fs::close(file);
        result
    }
    fn write_sync(&mut self, name: &str, bytes: &[u8]) -> Result<(), Error> {
        let file = fs::open(self.0, name, 1 | 2 | 8 | 16)
            .map_err(|e| storage_error(&format!("open {name}"), e))?;
        let result = fs::write(file, bytes)
            .map_err(|e| storage_error(&format!("write {name}"), e))
            .and_then(|_| {
                fs::sync_file(file).map_err(|e| storage_error(&format!("sync {name}"), e))
            });
        let _ = fs::close(file);
        result
    }
}
fn storage_error(stage: &str, status: fs_fidl::FsStatus) -> Error {
    bexos_userspace::log(&format!("prefsd: storage {stage}: {status:?}\n"));
    Error::Storage
}
fn directory(vfsd: Channel, uid: u64, id: &str, fingerprint: u64) -> Result<Directory, Error> {
    if !crate::service::valid_package(id) {
        return Err(Error::Malformed);
    }
    let home = if uid == 0 {
        vfs::get_system_data_directory(vfsd, "bexos.service.prefsd")
    } else {
        vfs::get_user_home_directory(vfsd, uid)
    }
    .map_err(|_| Error::Storage)?;
    let home = Directory(home);
    let prefs = Directory(
        fs::open(home.0, "prefs", 1 | 2 | 8 | 32)
            .map_err(|e| storage_error("prefs directory", e))?,
    );
    let package = Directory(
        fs::open(prefs.0, id, 1 | 2 | 8 | 32).map_err(|e| storage_error("package directory", e))?,
    );
    Ok(Directory(
        fs::open(package.0, &format!("{fingerprint:016x}"), 1 | 2 | 8 | 32)
            .map_err(|e| storage_error("schema directory", e))?,
    ))
}
fn operator_directory(vfsd: Channel, key: &str) -> Result<Directory, Error> {
    let root = Directory(
        vfs::get_system_data_directory(vfsd, "bexos.service.prefsd").map_err(|_| Error::Storage)?,
    );
    let parent =
        Directory(fs::open(root.0, "operators", 1 | 2 | 8 | 32).map_err(|_| Error::Storage)?);
    let name = key
        .as_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let mut current = parent;
    for component in name.as_bytes().chunks(128) {
        let name = core::str::from_utf8(component).map_err(|_| Error::Malformed)?;
        current = Directory(fs::open(current.0, name, 1 | 2 | 8 | 32).map_err(|_| Error::Storage)?);
    }
    Ok(current)
}
pub fn load_operator(vfsd: Channel, service: &mut Service, key: &str) -> Result<(), Error> {
    if service.operators.contains_key(key) {
        return Ok(());
    }
    let p = service.package(key)?.clone();
    let Some(record) = storage::load(&mut operator_directory(vfsd, key)?)? else {
        return Ok(());
    };
    let table = ConfigTable::parse(&record.bytes).map_err(|_| Error::Storage)?;
    let values = p
        .schema
        .decode_table(table.get_bytes("values").map_err(|_| Error::Storage)?)?;
    let mut locks = alloc::collections::BTreeSet::new();
    let mut r = Decoder::new(table.get_bytes("locks").map_err(|_| Error::Storage)?);
    let count = r.word().map_err(|_| Error::Storage)?;
    if count > 256 {
        return Err(Error::Storage);
    }
    for _ in 0..count {
        locks.insert(r.text(64).map_err(|_| Error::Storage)?.into());
    }
    r.finish().map_err(|_| Error::Storage)?;
    p.schema.validate_locks(&locks)?;
    service.operators.insert(
        key.into(),
        Operator {
            generation: record.generation,
            values,
            locks,
        },
    );
    Ok(())
}
pub fn load_user(vfsd: Channel, service: &mut Service, key: &str, uid: u64) -> Result<bool, Error> {
    let p = service.package(key)?.clone();
    let prefs_key = Service::prefs_key(&p, uid);
    if service.preferences.contains_key(&prefs_key) {
        return Ok(true);
    }
    let mut dir = directory(vfsd, uid, &p.id, p.schema.fingerprint())?;
    let (prefs, persisted) = if let Some(record) = storage::load(&mut dir)? {
        (decode_user(&p, &mut dir, &record)?, true)
    } else {
        (Preferences::default(), false)
    };
    service.preferences.insert(prefs_key, prefs);
    Ok(persisted)
}
pub fn persist_user(vfsd: Channel, service: &Service, key: &str, uid: u64) -> Result<(), Error> {
    let p = service.package(key)?;
    let prefs = service
        .preferences
        .get(&Service::prefs_key(p, uid))
        .ok_or(Error::Storage)?;
    let mut dir = directory(vfsd, uid, &p.id, p.schema.fingerprint())?;
    let old = storage::load(&mut dir)?;
    let generation = old.as_ref().map_or(0, |r| r.generation);
    if generation == prefs.generation {
        if generation == 0 {
            return Ok(());
        }
        storage::commit(&mut dir, generation - 1, &encode_user_record(p, prefs)?)?;
        return Ok(());
    }
    if generation >= prefs.generation {
        return Err(Error::Conflict);
    }
    storage::commit(&mut dir, generation, &encode_user_record(p, prefs)?)?;
    Ok(())
}
fn encode_revisions(prefs: &Preferences) -> Vec<u8> {
    let mut w = Encoder::new();
    w.word(prefs.generation);
    w.word(prefs.revisions.len() as u64);
    for (k, v) in &prefs.revisions {
        w.text(k);
        w.word(*v);
    }
    w.finish()
}
fn encode_user_record(p: &crate::service::Package, prefs: &Preferences) -> Result<Vec<u8>, Error> {
    let values = p.schema.encode_table(&prefs.values, prefs.generation)?;
    let revisions = encode_revisions(prefs);
    encode_config_v2(
        p.schema.fingerprint(),
        prefs.generation,
        &[
            ("values", ConfigType::Bytes, &values),
            ("revisions", ConfigType::Bytes, &revisions),
        ],
    )
    .map_err(|_| Error::Bounds)
}
pub fn persist_mutation(
    vfsd: Channel,
    candidate: &Service,
    mutation: &Mutation,
) -> Result<(), Error> {
    match mutation {
        Mutation::User {
            uid,
            package,
            fingerprint,
            ..
        } => {
            let key = candidate
                .packages
                .values()
                .find(|p| p.id == *package && p.schema.fingerprint() == *fingerprint)
                .ok_or(Error::UnknownField)?
                .key
                .clone();
            persist_user(vfsd, candidate, &key, *uid)
        }
        Mutation::Operator { package, .. } => {
            let p = candidate.package(package)?;
            let o = candidate.operators.get(package).ok_or(Error::Storage)?;
            let values = p.schema.encode_table(&o.values, o.generation)?;
            let mut w = Encoder::new();
            w.word(o.locks.len() as u64);
            for lock in &o.locks {
                w.text(lock);
            }
            let locks = w.finish();
            let blob = encode_config_v2(
                p.schema.fingerprint(),
                o.generation,
                &[
                    ("values", ConfigType::Bytes, &values),
                    ("locks", ConfigType::Bytes, &locks),
                ],
            )
            .map_err(|_| Error::Bounds)?;
            let mut dir = operator_directory(vfsd, package)?;
            let prior = storage::load(&mut dir)?.map_or(0, |r| r.generation);
            let expected = if prior == o.generation {
                o.generation - 1
            } else {
                prior
            };
            storage::commit(&mut dir, expected, &blob)?;
            Ok(())
        }
    }
}

fn decode_user(
    p: &crate::service::Package,
    dir: &mut Directory,
    record: &storage::Record,
) -> Result<Preferences, Error> {
    let table = ConfigTable::parse(&record.bytes).map_err(|_| Error::Storage)?;
    if let Some(prefs) = decode_wrapped_user(p, &table)? {
        return Ok(prefs);
    }
    let values = p.schema.decode_table(&record.bytes)?;
    p.schema
        .validate_assignments(&values, true, &Default::default())?;
    // Pre-wrapper records kept the operator revision checkpoint beside the
    // committed preference table. Keep reading them for recovery compatibility.
    let revisions = dir
        .read(&format!("{}.revisions", record.slot))?
        .ok_or(Error::Storage)?;
    let map = decode_revisions(&revisions, record.generation)?;
    Ok(Preferences {
        generation: record.generation,
        values,
        revisions: map,
    })
}
fn decode_revisions(bytes: &[u8], generation: u64) -> Result<BTreeMap<String, u64>, Error> {
    let mut r = Decoder::new(bytes);
    if r.word().map_err(|_| Error::Storage)? != generation {
        return Err(Error::Storage);
    }
    let count = r.word().map_err(|_| Error::Storage)?;
    if count > 256 {
        return Err(Error::Storage);
    }
    let mut map = BTreeMap::new();
    for _ in 0..count {
        map.insert(
            r.text(128).map_err(|_| Error::Storage)?.into(),
            r.word().map_err(|_| Error::Storage)?,
        );
    }
    r.finish().map_err(|_| Error::Storage)?;
    Ok(map)
}
fn decode_wrapped_user(
    p: &crate::service::Package,
    table: &ConfigTable<'_>,
) -> Result<Option<Preferences>, Error> {
    let Ok(values) = table.get_bytes("values") else {
        return Ok(None);
    };
    let Ok(revisions) = table.get_bytes("revisions") else {
        return Ok(None);
    };
    let values = p.schema.decode_table(values)?;
    p.schema
        .validate_assignments(&values, true, &Default::default())?;
    Ok(Some(Preferences {
        generation: table.generation(),
        values,
        revisions: decode_revisions(revisions, table.generation())?,
    }))
}
// The candidate was synchronized before a commit-marker error. Reload it after
// a user lock cleared plaintext caches, then retry the same durable decision.
pub fn restore_candidate_user(vfsd: Channel, service: &mut Service) -> Result<(), Error> {
    let pending = service.pending.as_ref().ok_or(Error::Busy)?;
    let Mutation::User {
        uid,
        package,
        fingerprint,
        ..
    } = &pending.mutation
    else {
        return Ok(());
    };
    if service.locked.contains(uid) {
        return Err(Error::AccessDenied);
    }
    let key = (*uid, package.clone(), *fingerprint);
    if service.preferences.contains_key(&key) {
        return Ok(());
    }
    let generation = pending.response_generation;
    let p = service
        .packages
        .values()
        .find(|p| p.id == *package && p.schema.fingerprint() == *fingerprint)
        .ok_or(Error::UnknownField)?
        .clone();
    let mut dir = directory(vfsd, *uid, package, *fingerprint)?;
    for slot in 0..2 {
        if let Some(bytes) = dir.read(&format!("{slot}.bexpref"))? {
            if ConfigTable::parse(&bytes).is_ok_and(|t| t.generation() == generation) {
                let record = storage::Record {
                    generation,
                    bytes,
                    slot,
                };
                let prefs = decode_user(&p, &mut dir, &record)?;
                service.preferences.insert(key, prefs);
                return Ok(());
            }
        }
    }
    Err(Error::Storage)
}
