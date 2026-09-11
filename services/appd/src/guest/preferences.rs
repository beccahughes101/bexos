//! Authoritative package registration and preference-backed launch snapshots.
use super::*;
use bexos_component_config::schema::Error;
use bexos_userspace::preferences::{self as rpc, wire as p};
use p::FidlDecode;

pub const PACKAGE: &str = "bexos.service.prefsd";
pub struct Connection(pub Channel);
impl Drop for Connection {
    fn drop(&mut self) {
        let _ = Memory::close(self.0.0);
    }
}
pub fn connect(services: &[state::ManagedService]) -> Result<Connection, Error> {
    let provider = services
        .iter()
        .find(|s| s.package == PACKAGE)
        .ok_or(Error::UnknownField)?;
    let (client, server) = Channel::pair().map_err(|_| Error::Storage)?;
    if Channel(provider.manager).send(b"bexos.preferences.Admin|PreferencesAdmin|Private|10,11,12,13,14,15,16||bexos.platform.appd|0|fg",&[server.0]).is_err(){let _=Memory::close(client.0);let _=Memory::close(server.0);return Err(Error::Storage);}
    Ok(Connection(client))
}
fn file(root: Channel, path: &str) -> Result<Vec<u8>, Error> {
    if root.0 == 0 {
        return Ok(Vec::new());
    }
    let f = match fs::open(root, path, 1) {
        Ok(f) => f,
        Err(fs_fidl::FsStatus::NotFound) => return Ok(Vec::new()),
        Err(_) => return Err(Error::Storage),
    };
    let result = (|| {
        let size = fs::attributes(f).map_err(|_| Error::Storage)?.size_bytes;
        if size > MAX_CONFIG_SNAPSHOT_LEN as u64 {
            return Err(Error::Bounds);
        }
        let mut bytes = Vec::new();
        while bytes.len() < (size as usize) {
            let chunk = fs::read(f, size - bytes.len() as u64).map_err(|_| Error::Storage)?;
            if chunk.is_empty() {
                return Err(Error::Storage);
            }
            bytes.extend(chunk);
        }
        Ok(bytes)
    })();
    let _ = fs::close(f);
    result
}
pub fn register(
    connection: Channel,
    record: &bexos_app_registry::AppRecord,
    root: Channel,
    legacy: &[state::ComponentConfigRecord],
) -> Result<(), Error> {
    let manifest = Manifest::decode(&record.manifest_bytes).map_err(|_| Error::InvalidSchema)?;
    let schema = manifest.config_schema.encode();
    let baseline = file(root, "config/component.bexconfig")?;
    let policy = file(root, "config/component.bexpolicy")?;
    let old = legacy.iter().find(|r| r.package == record.package_key());
    let m = rpc::call(
        connection,
        10,
        &p::PreferencesAdminRegisterPackageRequest {
            package_key: &record.package_key(),
            package_id: &record.package_id,
            schema: &schema,
            baseline: &baseline,
            policy: &policy,
            legacy_generation: old.map_or(0, |r| r.generation),
            legacy_config: old.map_or(&[], |r| r.bytes.as_slice()),
        },
    )?;
    let hs = rpc::refs(&m);
    let r = p::PreferencesAdminRegisterPackageResponse::decode(&m.bytes, &hs)
        .map_err(|_| Error::Malformed)?;
    check(r.status)
}
pub fn check(status: p::Status) -> Result<(), Error> {
    match status {
        p::Status::Ok | p::Status::CommittedPending => Ok(()),
        p::Status::Busy => Err(Error::Busy),
        p::Status::Conflict => Err(Error::Conflict),
        p::Status::AccessDenied => Err(Error::AccessDenied),
        p::Status::NotFound => Err(Error::UnknownField),
        p::Status::Rejected => Err(Error::Rejected),
        p::Status::Timeout => Err(Error::Timeout),
        _ => Err(Error::Storage),
    }
}
pub fn launch(
    services: &[state::ManagedService],
    root: Channel,
    record: &bexos_app_registry::AppRecord,
    uid: u64,
    legacy: &[state::ComponentConfigRecord],
) -> Result<(Option<(u64, u64)>, Option<Channel>), Error> {
    let manifest = Manifest::decode(&record.manifest_bytes).map_err(|_| Error::InvalidSchema)?;
    if manifest.config_schema.fields.is_empty() {
        return Ok((None, None));
    }
    if record.package_id == PACKAGE || !services.iter().any(|s| s.package == PACKAGE) {
        if manifest.config_schema.has_preferences() {
            return Err(Error::Busy);
        }
        let base = file(root, "config/component.bexconfig")?;
        let values = if base.is_empty() {
            Default::default()
        } else {
            manifest.config_schema.decode_table(&base)?
        };
        let values = manifest.config_schema.resolve(
            &values,
            &Default::default(),
            &Default::default(),
            &Default::default(),
        )?;
        let bytes = manifest.config_schema.encode_table(&values, 0)?;
        return Ok((
            Some((rpc::read_only_vmo(&bytes)?, bytes.len() as u64)),
            None,
        ));
    }
    let connection = connect(services)?;
    // Hold this launch while an existing transaction completes. The provider's
    // asynchronous event loop never calls back into appd.
    let start = bexos_userspace::syscall::ticks();
    loop {
        match register(connection.0, record, root, legacy) {
            Err(Error::Busy)
                if bexos_userspace::syscall::ticks().saturating_sub(start)
                    < bexos_userspace::syscall::frequency() * 300 =>
            {
                bexos_userspace::yield_now();
                continue;
            }
            result => {
                result?;
                break;
            }
        }
    }
    let key = record.package_key();
    loop {
        let m = rpc::call(
            connection.0,
            11,
            &p::PreferencesAdminResolveRequest {
                package_key: &key,
                uid,
            },
        )?;
        let hs = rpc::refs(&m);
        let r = p::PreferencesAdminResolveResponse::decode(&m.bytes, &hs)
            .map_err(|_| Error::Malformed)?;
        if r.status == p::Status::Busy
            && bexos_userspace::syscall::ticks().saturating_sub(start)
                < bexos_userspace::syscall::frequency() * 300
        {
            bexos_userspace::yield_now();
            continue;
        }
        check(r.status)?;
        let raw = r.config.first().ok_or(Error::MissingField)?.raw;
        let len = r.config_len;
        let (client, server) = Channel::pair().map_err(|_| {
            let _ = Memory::close(raw);
            Error::Storage
        })?;
        let result = (|| {
            let m = rpc::call(
                connection.0,
                12,
                &p::PreferencesAdminAttachReceiverRequest {
                    package_key: &key,
                    uid,
                    receiver: p::HandleRef { raw: server.0 },
                },
            )?;
            let hs = rpc::refs(&m);
            let r = p::PreferencesAdminAttachReceiverResponse::decode(&m.bytes, &hs)
                .map_err(|_| Error::Malformed)?;
            check(r.status)
        })();
        if let Err(e) = result {
            let _ = Memory::close(raw);
            let _ = Memory::close(client.0);
            if e == Error::Busy
                && bexos_userspace::syscall::ticks().saturating_sub(start)
                    < bexos_userspace::syscall::frequency() * 300
            {
                bexos_userspace::yield_now();
                continue;
            }
            return Err(e);
        }
        return Ok((Some((raw, len)), Some(client)));
    }
}
pub fn poll_registration(state: &mut state::AppdState) {
    let Some(provider) = state.services.iter().find(|s| s.package == PACKAGE) else {
        return;
    };
    let instance = (provider.manager, provider.generation);
    if state.preference_provider != instance {
        state.preference_provider = instance;
        state.preference_registered.clear();
    }
    let candidate = state
        .registry
        .list_packages()
        .iter()
        .find(|r| {
            r.package_id != PACKAGE && !state.preference_registered.contains(&r.package_key())
        })
        .cloned();
    let Some(record) = candidate else {
        return;
    };
    let schema = match Manifest::decode(&record.manifest_bytes) {
        Ok(m) => m.config_schema,
        Err(_) => return,
    };
    if schema.fields.is_empty() {
        state.preference_registered.insert(record.package_key());
        return;
    }
    let Ok(root) = package_root(state.vfsd, &record) else {
        return;
    };
    let result = connect(&state.services)
        .and_then(|c| register(c.0, &record, root, &state.component_configs));
    let _ = Memory::close(root.0);
    if result.is_ok() {
        state.preference_registered.insert(record.package_key());
    }
}

pub fn operator(
    state: &state::AppdState,
    selector: &str,
    operation: u32,
    expected: u64,
    bytes: &[u8],
    names: &[&str],
) -> Result<(p::Status, u64, Vec<u8>, Vec<String>), Error> {
    let record = state
        .registry
        .record(selector)
        .map_err(|_| Error::UnknownField)?;
    let c = connect(&state.services)?;
    let root = package_root(state.vfsd, &record).map_err(|_| Error::Storage)?;
    let result = register(c.0, &record, root, &state.component_configs);
    let _ = Memory::close(root.0);
    result?;
    let m = rpc::call(
        c.0,
        13,
        &p::PreferencesAdminOperatorRequest {
            package_key: &record.package_key(),
            operation,
            expected_generation: expected,
            config: bytes,
            names: p::WireStringVector::from_slice(names),
        },
    )?;
    let hs = rpc::refs(&m);
    let r =
        p::PreferencesAdminOperatorResponse::decode(&m.bytes, &hs).map_err(|_| Error::Malformed)?;
    let bytes = if let Some(h) = r.config.first() {
        rpc::read_vmo(h.raw, r.config_len)?
    } else {
        Vec::new()
    };
    let locks = (0..r.locks.len())
        .map(|i| {
            r.locks
                .get(i)
                .map(String::from)
                .map_err(|_| Error::Malformed)
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok((r.status, r.generation, bytes, locks))
}
pub fn lifecycle_status(status: p::Status) -> lifecycle::ComponentConfigStatus {
    match status {
        p::Status::Ok | p::Status::CommittedPending => lifecycle::ComponentConfigStatus::Ok,
        p::Status::Conflict => lifecycle::ComponentConfigStatus::Conflict,
        p::Status::Busy => lifecycle::ComponentConfigStatus::Busy,
        p::Status::Rejected => lifecycle::ComponentConfigStatus::Rejected,
        p::Status::Timeout => lifecycle::ComponentConfigStatus::Timeout,
        p::Status::NotFound => lifecycle::ComponentConfigStatus::NotFound,
        p::Status::InvalidArgs | p::Status::AccessDenied => {
            lifecycle::ComponentConfigStatus::InvalidArgs
        }
        _ => lifecycle::ComponentConfigStatus::Storage,
    }
}
pub fn mutate(
    state: &state::AppdState,
    selector: &str,
    operation: u32,
    expected: u64,
    bytes: &[u8],
    names: &[&str],
) -> (lifecycle::ComponentConfigStatus, u64, String) {
    match operator(state, selector, operation, expected, bytes, names) {
        Ok((s, g, _, _)) => (
            lifecycle_status(s),
            g,
            if s == p::Status::CommittedPending {
                "committed; receiver delivery pending".into()
            } else {
                alloc::format!("{s:?}")
            },
        ),
        Err(e) => (
            lifecycle_status(rpc::status(e)),
            expected,
            alloc::format!("{e}"),
        ),
    }
}

pub fn debug_request(
    state: &state::AppdState,
    bytes: &[u8],
) -> bexos_debug_wire::PreferencesResponse {
    match debug_request_inner(state, bytes) {
        Ok(r) => r,
        Err(e) => bexos_debug_wire::PreferencesResponse {
            status: rpc::status(e) as i32,
            message: alloc::format!("{e}"),
            ..Default::default()
        },
    }
}
fn debug_request_inner(
    state: &state::AppdState,
    bytes: &[u8],
) -> Result<bexos_debug_wire::PreferencesResponse, Error> {
    let q = bexos_debug_wire::decode_preferences_request(bytes).map_err(|_| Error::Malformed)?;
    if matches!(q.operation, 3 | 4 | 5) {
        let names = q.names.iter().map(String::as_str).collect::<Vec<_>>();
        let (s, g, config, locks) = operator(
            state,
            &q.package_id,
            if q.operation == 5 { 0 } else { q.operation },
            q.expected_generation,
            &[],
            &names,
        )?;
        return Ok(bexos_debug_wire::PreferencesResponse {
            status: s as i32,
            generation: g,
            config,
            locks,
            message: alloc::format!("{s:?}"),
            schema: Vec::new(),
        });
    }
    let record = state
        .registry
        .record(&q.package_id)
        .map_err(|_| Error::UnknownField)?;
    let admin = connect(&state.services)?;
    let root = package_root(state.vfsd, &record).map_err(|_| Error::Storage)?;
    let result = register(admin.0, &record, root, &state.component_configs);
    let _ = Memory::close(root.0);
    result?;
    let provider = state
        .services
        .iter()
        .find(|s| s.package == PACKAGE)
        .ok_or(Error::UnknownField)?;
    let (client, server) = Channel::pair().map_err(|_| Error::Storage)?;
    let client = Connection(client);
    Channel(provider.manager).send(b"bexos.preferences.UserPreferences|UserPreferences|ManageUserPreferences|4||bexos.platform.appd|0|fg",&[server.0]).map_err(|_|Error::Storage)?;
    let schema = Manifest::decode(&record.manifest_bytes)
        .map_err(|_| Error::InvalidSchema)?
        .config_schema;
    let values = if q.config.is_empty() {
        Default::default()
    } else {
        schema.decode_table(&q.config)?
    };
    let assignments = values
        .iter()
        .map(|(key, v)| p::Assignment {
            key,
            value: match v {
                bexos_component_config::schema::Value::Bool(value) => {
                    p::ConfigValue::BoolValue(p::BoolConfig { value: *value })
                }
                bexos_component_config::schema::Value::Uint32(value) => {
                    p::ConfigValue::Uint32Value(p::Uint32Config { value: *value })
                }
                bexos_component_config::schema::Value::Uint64(value) => {
                    p::ConfigValue::Uint64Value(p::Uint64Config { value: *value })
                }
                bexos_component_config::schema::Value::String(value) => {
                    p::ConfigValue::StringValue(p::StringConfig { value })
                }
                bexos_component_config::schema::Value::Bytes(value) => {
                    p::ConfigValue::BytesValue(p::BytesConfig { value })
                }
            },
        })
        .collect::<Vec<_>>();
    let m = rpc::call(
        client.0,
        4,
        &p::UserPreferencesManageRequest {
            package_id: &record.package_key(),
            uid: q.uid,
            operation: q.operation,
            expected_generation: q.expected_generation,
            assignments: p::WireVector::from_slice(&assignments),
        },
    )?;
    let hs = rpc::refs(&m);
    let r =
        p::UserPreferencesManageResponse::decode(&m.bytes, &hs).map_err(|_| Error::Malformed)?;
    let config = if let Some(h) = r.config.first() {
        rpc::read_vmo(h.raw, r.config_len)?
    } else {
        Vec::new()
    };
    let locks = (0..r.locks.len())
        .map(|i| {
            r.locks
                .get(i)
                .map(String::from)
                .map_err(|_| Error::Malformed)
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(bexos_debug_wire::PreferencesResponse {
        status: r.status as i32,
        generation: r.generation,
        config,
        schema: r.schema.to_vec(),
        locks,
        message: alloc::format!("{:?}", r.status),
    })
}

pub fn freeze(state: &state::AppdState, frozen: bool) -> Result<(), Error> {
    if !state.services.iter().any(|s| s.package == PACKAGE) {
        return Ok(());
    }
    let c = connect(&state.services)?;
    let m = rpc::call(c.0, 15, &p::PreferencesAdminFreezeRequest { frozen })?;
    let hs = rpc::refs(&m);
    let r =
        p::PreferencesAdminFreezeResponse::decode(&m.bytes, &hs).map_err(|_| Error::Malformed)?;
    check(r.status)
}

pub fn reconcile(state: &mut state::AppdState) -> Result<(), Error> {
    if !state.services.iter().any(|s| s.package == PACKAGE) {
        return Ok(());
    }
    let keys = state
        .registry
        .list_packages()
        .iter()
        .map(|r| r.package_key())
        .collect::<Vec<_>>();
    let names = keys.iter().map(String::as_str).collect::<Vec<_>>();
    let c = connect(&state.services)?;
    let m = rpc::call(
        c.0,
        16,
        &p::PreferencesAdminReconcileRequest {
            installed: p::WireStringVector::from_slice(&names),
        },
    )?;
    let hs = rpc::refs(&m);
    let r = p::PreferencesAdminReconcileResponse::decode(&m.bytes, &hs)
        .map_err(|_| Error::Malformed)?;
    check(r.status)?;
    state.preference_registered.retain(|key| keys.contains(key));
    Ok(())
}

pub fn bind_identity(binding: &mut BoundCapability, record: &bexos_app_registry::AppRecord) {
    if binding.service_name == "bexos.preferences.UserPreferences" {
        binding
            .permission_values
            .retain(|v| !v.starts_with("caller-package-key:"));
        binding.permission_values.push(alloc::format!(
            "caller-package-key:{}",
            record.package_key()
        ));
    }
}

// appd is a BootFS component: its registered schema also owns the durable shell selectors.
fn package_root(
    vfsd: Channel,
    record: &bexos_app_registry::AppRecord,
) -> Result<Channel, fs_fidl::FsStatus> {
    // The appd package is retained as a manifest after BootFS reclamation;
    // there is no appd archive to mount in the base image.
    if record.package_id == state::APPD_PACKAGE {
        return Ok(Channel(0));
    }
    vfs::get_package_directory(vfsd, &record.archive_id())
}
pub fn resolve_shell_selection(
    registry: &MemoryAppRegistry,
    services: &[state::ManagedService],
    vfsd: Channel,
    legacy: &[state::ComponentConfigRecord],
    uid: u64,
    key: &str,
) -> Result<String, Error> {
    let record = registry
        .record(state::APPD_PACKAGE)
        .map_err(|_| Error::UnknownField)?;
    let root = package_root(vfsd, record).map_err(|_| Error::Storage)?;
    let c = connect(services)?;
    let result = register(c.0, record, root, legacy);
    if root.0 != 0 {
        let _ = Memory::close(root.0);
    }
    result?;
    let m = rpc::call(
        c.0,
        11,
        &p::PreferencesAdminResolveRequest {
            package_key: &record.package_key(),
            uid,
        },
    )?;
    let hs = rpc::refs(&m);
    let response =
        p::PreferencesAdminResolveResponse::decode(&m.bytes, &hs).map_err(|_| Error::Malformed)?;
    check(response.status)?;
    let h = response.config.first().ok_or(Error::MissingField)?;
    let bytes = rpc::read_vmo(h.raw, response.config_len)?;
    let schema = Manifest::decode(&record.manifest_bytes)
        .map_err(|_| Error::InvalidSchema)?
        .config_schema;
    match schema.decode_table(&bytes)?.get(key) {
        Some(bexos_component_config::schema::Value::String(value)) => Ok(value.clone()),
        _ => Err(Error::TypeMismatch),
    }
}

pub fn shell_selection(state: &state::AppdState, uid: u64, key: &str) -> Result<String, Error> {
    resolve_shell_selection(
        &state.registry,
        &state.services,
        state.vfsd,
        &state.component_configs,
        uid,
        key,
    )
}
