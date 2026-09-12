use crate::{
    binding::Client,
    runtime::{Runtime, now_ms},
    service::{Mutation, Observer, Package, Service, ThemeObserver},
};
use alloc::{
    collections::{BTreeMap, BTreeSet},
    string::String,
    vec::Vec,
};
use bexos_component_config::schema::{Assignments, Error, Schema};
use bexos_ui_theme::{PACKAGE_ID, ThemePreferences};
use bexos_userspace::{Channel, Memory, preferences as rpc};
use preferences_fidl::*;

pub fn mutation_reply(channel: u64, ordinal: u64, status: Status, generation: u64) {
    if channel == 0 {
        return;
    }
    let channel = Channel(channel);
    match ordinal {
        2 => {
            let _ = rpc::reply(
                channel,
                &UserPreferencesSetPreferenceResponse { status, generation },
            );
        }
        3 => {
            let _ = rpc::reply(
                channel,
                &UserPreferencesResetToDefaultsResponse { status, generation },
            );
        }
        4 => {
            let _ = rpc::reply(
                channel,
                &UserPreferencesManageResponse {
                    status,
                    generation,
                    config: &[],
                    config_len: 0,
                    schema: &[],
                    locks: WireStringVector::from_slice(&[]),
                },
            );
        }
        13 => {
            let _ = rpc::reply(
                channel,
                &PreferencesAdminOperatorResponse {
                    status,
                    generation,
                    config: &[],
                    config_len: 0,
                    locks: WireStringVector::from_slice(&[]),
                },
            );
        }
        _ => {}
    }
}
pub fn poll(runtime: &mut Runtime) {
    let clients = runtime.clients.clone();
    for client in clients {
        if runtime
            .service
            .pending
            .as_ref()
            .is_some_and(|p| p.reply_channel == client.channel)
        {
            continue;
        }
        match Channel(client.channel).try_recv() {
            Ok(message) => {
                let ordinal = message
                    .bytes
                    .get(..8)
                    .map(|b| u64::from_le_bytes(b.try_into().unwrap()))
                    .unwrap_or(0);
                if !client.methods.contains(&ordinal) {
                    for h in message.handles {
                        let _ = Memory::close(h);
                    }
                    let _ = Memory::close(client.channel);
                    runtime.clients.retain(|c| c.channel != client.channel);
                    continue;
                }
                let expected_handles = if client.admin && ordinal == 12 {
                    1
                } else if client.theme && ordinal == 2 {
                    1
                } else {
                    0
                };
                if message.handles.len() != expected_handles {
                    for h in message.handles {
                        let _ = Memory::close(h);
                    }
                    if client.theme {
                        theme_error_reply(client.channel, ordinal, Status::InvalidArgs);
                    } else {
                        error_reply(client.channel, ordinal, Status::InvalidArgs);
                    }
                    continue;
                }
                let hs = rpc::refs(&message);
                let result = handle(
                    runtime,
                    &client,
                    ordinal,
                    message.bytes.get(8..).unwrap_or(&[]),
                    &hs,
                );
                if let Err(error) = result {
                    if error == Error::Storage {
                        bexos_userspace::log(&alloc::format!(
                            "prefsd: request ordinal={ordinal} storage failure\n"
                        ));
                    }
                    runtime
                        .service
                        .observers
                        .retain(|o| !message.handles.contains(&o.channel));
                    for h in message.handles {
                        let _ = Memory::close(h);
                    }
                    if client.theme {
                        theme_error_reply(client.channel, ordinal, rpc::status(error));
                    } else {
                        error_reply(client.channel, ordinal, rpc::status(error));
                    }
                }
            }
            Err(kernel_fidl::Status::ErrPeerClosed) => {
                let _ = Memory::close(client.channel);
                runtime.clients.retain(|c| c.channel != client.channel);
            }
            Err(_) => {}
        }
    }
}
fn error_reply(channel: u64, ordinal: u64, status: Status) {
    let channel = Channel(channel);
    match ordinal {
        1 => {
            let _ = rpc::reply(
                channel,
                &UserPreferencesGetPreferencesResponse {
                    status,
                    generation: 0,
                    config: &[],
                    config_len: 0,
                    schema: &[],
                    locks: WireStringVector::from_slice(&[]),
                },
            );
        }
        10 => {
            let _ = rpc::reply(channel, &PreferencesAdminRegisterPackageResponse { status });
        }
        11 => {
            let _ = rpc::reply(
                channel,
                &PreferencesAdminResolveResponse {
                    status,
                    generation: 0,
                    config: &[],
                    config_len: 0,
                },
            );
        }
        12 => {
            let _ = rpc::reply(channel, &PreferencesAdminAttachReceiverResponse { status });
        }
        16 => {
            let _ = rpc::reply(channel, &PreferencesAdminReconcileResponse { status });
        }
        15 => {
            let _ = rpc::reply(channel, &PreferencesAdminFreezeResponse { status });
        }
        14 => {
            let _ = rpc::reply(channel, &PreferencesAdminUserStateResponse { status });
        }
        _ => mutation_reply(channel.0, ordinal, status, 0),
    }
}
fn theme_error_reply(channel: u64, ordinal: u64, status: Status) {
    let metadata = ThemeMetadata {
        generation: 0,
        color_scheme: "",
        text_scale_percent: 100,
        reduce_motion: false,
    };
    let channel = Channel(channel);
    match ordinal {
        1 => {
            let _ = rpc::reply(
                channel,
                &ThemeManagerGetThemeResponse {
                    status,
                    metadata,
                    stylesheet: &[],
                    stylesheet_len: 0,
                },
            );
        }
        2 => {
            let _ = rpc::reply(
                channel,
                &ThemeManagerWatchThemeResponse {
                    status,
                    metadata,
                    stylesheet: &[],
                    stylesheet_len: 0,
                },
            );
        }
        _ => {}
    }
}
fn start_reply(runtime: &mut Runtime, client: &Client, ordinal: u64) -> Result<(), Error> {
    let p = runtime.service.pending.as_mut().ok_or(Error::Busy)?;
    p.reply_channel = client.channel;
    p.reply_ordinal = ordinal;
    if let Err(e) = runtime.begin_delivery() {
        runtime.abort(rpc::status(e));
    }
    Ok(())
}
fn resolve_key(runtime: &Runtime, selector: &str) -> Result<String, Error> {
    Ok(runtime.service.package(selector)?.key.clone())
}
fn resolve_caller_key(runtime: &Runtime, client: &Client, selector: &str) -> Result<String, Error> {
    if selector != client.package && selector != client.package_key {
        return Err(Error::AccessDenied);
    }
    resolve_key(runtime, &client.package_key)
}
fn handle(
    runtime: &mut Runtime,
    client: &Client,
    ordinal: u64,
    bytes: &[u8],
    hs: &[HandleRef],
) -> Result<(), Error> {
    let channel = Channel(client.channel);
    if client.theme {
        return handle_theme(runtime, client, ordinal, bytes, hs);
    }
    if client.admin {
        match ordinal {
            16 => {
                let q = PreferencesAdminReconcileRequest::decode(bytes, hs)
                    .map_err(|_| Error::Malformed)?;
                if runtime.service.pending.is_some() {
                    return Err(Error::Busy);
                }
                let keys = (0..q.installed.len())
                    .map(|i| {
                        q.installed
                            .get(i)
                            .map(String::from)
                            .map_err(|_| Error::Malformed)
                    })
                    .collect::<Result<BTreeSet<_>, _>>()?;
                let removed = runtime
                    .service
                    .packages
                    .keys()
                    .filter(|k| !keys.contains(*k))
                    .cloned()
                    .collect::<BTreeSet<_>>();
                for o in runtime
                    .service
                    .observers
                    .iter()
                    .filter(|o| removed.contains(&o.package))
                {
                    let _ = Memory::close(o.channel);
                }
                runtime
                    .service
                    .observers
                    .retain(|o| !removed.contains(&o.package));
                runtime.service.packages.retain(|key, _| keys.contains(key));
                runtime
                    .service
                    .operators
                    .retain(|key, _| keys.contains(key));
                runtime.service.preferences.retain(|(_, id, fp), _| {
                    runtime
                        .service
                        .packages
                        .values()
                        .any(|p| p.id == *id && p.schema.fingerprint() == *fp)
                });
                let ids = runtime
                    .service
                    .packages
                    .values()
                    .map(|p| p.id.clone())
                    .collect::<BTreeSet<_>>();
                runtime.clients.retain(|c| {
                    if c.admin || c.manage || ids.contains(&c.package) {
                        true
                    } else {
                        let _ = Memory::close(c.channel);
                        false
                    }
                });
                rpc::reply(
                    channel,
                    &PreferencesAdminReconcileResponse { status: Status::Ok },
                )?;
            }
            15 => {
                let q = PreferencesAdminFreezeRequest::decode(bytes, hs)
                    .map_err(|_| Error::Malformed)?;
                if q.frozen {
                    if runtime.service.pending.as_ref().is_some_and(|p| {
                        p.transaction.phase == bexos_component_config::transaction::Phase::Resolving
                    }) {
                        return Err(Error::Busy);
                    }
                    runtime.abort(Status::Busy);
                }
                runtime.service.frozen = q.frozen;
                rpc::reply(
                    channel,
                    &PreferencesAdminFreezeResponse { status: Status::Ok },
                )?;
            }
            10 => {
                let q = PreferencesAdminRegisterPackageRequest::decode(bytes, hs)
                    .map_err(|_| Error::Malformed)?;
                let schema = Schema::decode(q.schema)?;
                schema.validate().map_err(|_| Error::InvalidSchema)?;
                let base = if q.baseline.is_empty() {
                    Assignments::new()
                } else {
                    schema.decode_table(q.baseline)?
                };
                let product_locks = rpc::decode_policy(&schema, q.policy)?;
                runtime.service.register(Package {
                    key: q.package_key.into(),
                    id: q.package_id.into(),
                    schema,
                    base,
                    product_locks,
                })?;
                crate::storage::load_operator(runtime.vfsd, &mut runtime.service, q.package_key)?;
                if q.legacy_generation > 0 && !runtime.service.operators.contains_key(q.package_key)
                {
                    let schema = &runtime.service.package(q.package_key)?.schema;
                    let values = if q.legacy_config.is_empty() {
                        Assignments::new()
                    } else {
                        schema.decode_table(q.legacy_config)?
                    };
                    let mut candidate = runtime.service.clone();
                    candidate.operators.insert(
                        q.package_key.into(),
                        crate::service::Operator {
                            generation: q.legacy_generation,
                            values: values.clone(),
                            locks: Default::default(),
                        },
                    );
                    crate::storage::persist_mutation(
                        runtime.vfsd,
                        &candidate,
                        &crate::service::Mutation::Operator {
                            package: q.package_key.into(),
                            values,
                            locks: Default::default(),
                        },
                    )?;
                    runtime.service = candidate;
                }
                rpc::reply(
                    channel,
                    &PreferencesAdminRegisterPackageResponse { status: Status::Ok },
                )?;
            }
            11 => {
                let q = PreferencesAdminResolveRequest::decode(bytes, hs)
                    .map_err(|_| Error::Malformed)?;
                runtime.load(q.package_key, q.uid)?;
                let (generation, b) = runtime.service.effective(q.package_key, q.uid)?;
                let raw = rpc::read_only_vmo(&b)?;
                rpc::reply(
                    channel,
                    &PreferencesAdminResolveResponse {
                        status: Status::Ok,
                        generation,
                        config: &[HandleRef { raw }],
                        config_len: b.len() as u64,
                    },
                )?;
            }
            12 => {
                let q = PreferencesAdminAttachReceiverRequest::decode(bytes, hs)
                    .map_err(|_| Error::Malformed)?;
                runtime.check_user(q.uid)?;
                runtime.service.package(q.package_key)?;
                if runtime.service.pending.is_some() {
                    return Err(Error::Busy);
                }
                if runtime.service.observers.len() >= 256 {
                    return Err(Error::Bounds);
                }
                runtime.service.observers.push(Observer {
                    channel: q.receiver.raw,
                    package: q.package_key.into(),
                    uid: q.uid,
                    registered: false,
                });
                rpc::reply(
                    channel,
                    &PreferencesAdminAttachReceiverResponse { status: Status::Ok },
                )?;
            }
            13 => {
                let q = PreferencesAdminOperatorRequest::decode(bytes, hs)
                    .map_err(|_| Error::Malformed)?;
                let key = resolve_key(runtime, q.package_key)?;
                crate::storage::load_operator(runtime.vfsd, &mut runtime.service, &key)?;
                let p = runtime.service.package(&key)?;
                let generation = runtime
                    .service
                    .operators
                    .get(&key)
                    .map_or(0, |o| o.generation);
                if runtime.service.pending.is_some() {
                    return Err(Error::Busy);
                }
                if q.operation == 0 {
                    let empty = Assignments::new();
                    let values = runtime
                        .service
                        .operators
                        .get(&key)
                        .map_or(&empty, |o| &o.values);
                    let b = p.schema.encode_table(values, generation)?;
                    let locks = runtime.service.locks(p);
                    let names = locks.iter().map(String::as_str).collect::<Vec<_>>();
                    let raw = rpc::read_only_vmo(&b)?;
                    rpc::reply(
                        channel,
                        &PreferencesAdminOperatorResponse {
                            status: Status::Ok,
                            generation,
                            config: &[HandleRef { raw }],
                            config_len: b.len() as u64,
                            locks: WireStringVector::from_slice(&names),
                        },
                    )?;
                } else if q.expected_generation != generation {
                    mutation_reply(client.channel, ordinal, Status::Conflict, generation);
                } else {
                    let values = if q.config.is_empty() {
                        Assignments::new()
                    } else {
                        p.schema.decode_table(q.config)?
                    };
                    let names = (0..q.names.len())
                        .map(|i| {
                            q.names
                                .get(i)
                                .map(String::from)
                                .map_err(|_| Error::Malformed)
                        })
                        .collect::<Result<BTreeSet<_>, _>>()?;
                    runtime.service.begin_operator(
                        &key,
                        q.expected_generation,
                        values,
                        names,
                        q.operation,
                        now_ms(),
                        runtime.timeout_ms,
                    )?;
                    start_reply(runtime, client, ordinal)?;
                }
            }
            14 => {
                let q = PreferencesAdminUserStateRequest::decode(bytes, hs)
                    .map_err(|_| Error::Malformed)?;
                if q.deleted {
                    runtime.delete_user(q.uid);
                } else if !q.unlocked {
                    runtime.lock(q.uid);
                } else {
                    runtime.check_user(q.uid)?;
                }
                rpc::reply(
                    channel,
                    &PreferencesAdminUserStateResponse { status: Status::Ok },
                )?;
            }
            _ => return Err(Error::AccessDenied),
        }
        return Ok(());
    }
    match ordinal {
        1 => {
            let q = UserPreferencesGetPreferencesRequest::decode(bytes, hs)
                .map_err(|_| Error::Malformed)?;
            let key = resolve_caller_key(runtime, client, q.package_id)?;
            runtime
                .service
                .authorize(&client.package, client.uid, false, &key, client.uid)?;
            runtime.load(&key, client.uid)?;
            get_reply(runtime, client, 1, &key, client.uid)?;
        }
        2 => {
            let q = UserPreferencesSetPreferenceRequest::decode(bytes, hs)
                .map_err(|_| Error::Malformed)?;
            let key = resolve_caller_key(runtime, client, q.package_id)?;
            runtime
                .service
                .authorize(&client.package, client.uid, false, &key, client.uid)?;
            runtime.load(&key, client.uid)?;
            let generation = runtime.service.effective(&key, client.uid)?.0;
            if generation != q.expected_generation {
                mutation_reply(client.channel, ordinal, Status::Conflict, generation);
            } else {
                runtime.service.begin_user(
                    &key,
                    client.uid,
                    q.expected_generation,
                    BTreeMap::from([(q.key.into(), rpc::value(&q.value))]),
                    false,
                    now_ms(),
                    runtime.timeout_ms,
                )?;
                start_reply(runtime, client, ordinal)?;
            }
        }
        3 => {
            let q = UserPreferencesResetToDefaultsRequest::decode(bytes, hs)
                .map_err(|_| Error::Malformed)?;
            let key = resolve_caller_key(runtime, client, q.package_id)?;
            runtime
                .service
                .authorize(&client.package, client.uid, false, &key, client.uid)?;
            runtime.load(&key, client.uid)?;
            let generation = runtime.service.effective(&key, client.uid)?.0;
            if generation != q.expected_generation {
                mutation_reply(client.channel, ordinal, Status::Conflict, generation);
            } else {
                runtime.service.begin_user(
                    &key,
                    client.uid,
                    q.expected_generation,
                    Assignments::new(),
                    true,
                    now_ms(),
                    runtime.timeout_ms,
                )?;
                start_reply(runtime, client, ordinal)?;
            }
        }
        4 if client.manage => {
            let q =
                UserPreferencesManageRequest::decode(bytes, hs).map_err(|_| Error::Malformed)?;
            let key = resolve_key(runtime, q.package_id)?;
            runtime.load(&key, q.uid)?;
            runtime
                .service
                .authorize(&client.package, client.uid, true, &key, q.uid)?;
            if q.operation == 0 {
                get_reply(runtime, client, 4, &key, q.uid)?;
            } else {
                let generation = runtime.service.effective(&key, q.uid)?.0;
                if generation != q.expected_generation {
                    mutation_reply(client.channel, ordinal, Status::Conflict, generation);
                } else {
                    if !matches!(q.operation, 1 | 2) {
                        return Err(Error::Malformed);
                    }
                    let mut values = Assignments::new();
                    for i in 0..q.assignments.len() {
                        let v = q.assignments.get(i).map_err(|_| Error::Malformed)?;
                        if values.insert(v.key.into(), rpc::value(&v.value)).is_some() {
                            return Err(Error::Malformed);
                        }
                    }
                    runtime.service.begin_user(
                        &key,
                        q.uid,
                        q.expected_generation,
                        values,
                        q.operation == 2,
                        now_ms(),
                        runtime.timeout_ms,
                    )?;
                    start_reply(runtime, client, ordinal)?;
                }
            }
        }
        _ => return Err(Error::AccessDenied),
    }
    Ok(())
}

fn handle_theme(
    runtime: &mut Runtime,
    client: &Client,
    ordinal: u64,
    bytes: &[u8],
    hs: &[HandleRef],
) -> Result<(), Error> {
    let channel = Channel(client.channel);
    match ordinal {
        1 => {
            ThemeManagerGetThemeRequest::decode(bytes, hs).map_err(|_| Error::Malformed)?;
            let (metadata, stylesheet) = theme_snapshot(runtime, client.uid)?;
            let raw = rpc::read_only_vmo(stylesheet.as_bytes())?;
            rpc::reply(
                channel,
                &ThemeManagerGetThemeResponse {
                    status: Status::Ok,
                    metadata,
                    stylesheet: &[HandleRef { raw }],
                    stylesheet_len: stylesheet.len() as u64,
                },
            )?;
        }
        2 => {
            let q =
                ThemeManagerWatchThemeRequest::decode(bytes, hs).map_err(|_| Error::Malformed)?;
            if runtime.service.theme_observers.len() >= 256 {
                return Err(Error::Bounds);
            }
            let (metadata, stylesheet) = theme_snapshot(runtime, client.uid)?;
            let generation = metadata.generation;
            let raw = rpc::read_only_vmo(stylesheet.as_bytes())?;
            let observer = ThemeObserver {
                channel: q.observer.raw,
                uid: client.uid,
                generation,
            };
            rpc::reply(
                channel,
                &ThemeManagerWatchThemeResponse {
                    status: Status::Ok,
                    metadata,
                    stylesheet: &[HandleRef { raw }],
                    stylesheet_len: stylesheet.len() as u64,
                },
            )?;
            runtime.service.theme_observers.push(observer);
        }
        _ => return Err(Error::AccessDenied),
    }
    Ok(())
}

fn theme_snapshot(
    runtime: &mut Runtime,
    uid: u64,
) -> Result<(ThemeMetadata<'static>, String), Error> {
    let key = resolve_key(runtime, PACKAGE_ID)?;
    if runtime.service.pending.is_none() {
        runtime.load(&key, uid)?;
    }
    theme_snapshot_from_service(&runtime.service, &key, uid)
}

fn theme_snapshot_from_service(
    service: &Service,
    key: &str,
    uid: u64,
) -> Result<(ThemeMetadata<'static>, String), Error> {
    let (generation, table) = service.effective(key, uid)?;
    let (_, prefs) = ThemePreferences::from_table(&table).map_err(|_| Error::Malformed)?;
    Ok((
        ThemeMetadata {
            generation,
            color_scheme: prefs.color_scheme.as_str(),
            text_scale_percent: prefs.text_scale_percent,
            reduce_motion: prefs.reduce_motion,
        },
        prefs.user_css(),
    ))
}

pub fn broadcast_theme_changes(service: &mut Service, mutation: &Mutation) {
    let Ok(key) = service.package(PACKAGE_ID).map(|p| p.key.clone()) else {
        return;
    };
    let observers = service.theme_observers.clone();
    for observer in observers {
        if !service.affected(mutation, &key, observer.uid) {
            continue;
        }
        let Ok((metadata, stylesheet)) = theme_snapshot_from_service(service, &key, observer.uid)
        else {
            continue;
        };
        if metadata.generation <= observer.generation {
            continue;
        }
        let generation = metadata.generation;
        let Ok(raw) = rpc::read_only_vmo(stylesheet.as_bytes()) else {
            continue;
        };
        let request = ThemeObserverOnThemeChangedRequest {
            metadata,
            stylesheet: HandleRef { raw },
            stylesheet_len: stylesheet.len() as u64,
        };
        if rpc::send(Channel(observer.channel), 1, &request).is_err() {
            let _ = Memory::close(raw);
            let _ = Memory::close(observer.channel);
            service
                .theme_observers
                .retain(|o| o.channel != observer.channel);
        } else if let Some(stored) = service
            .theme_observers
            .iter_mut()
            .find(|o| o.channel == observer.channel)
        {
            stored.generation = generation;
        }
    }
}
fn get_reply(
    runtime: &Runtime,
    client: &Client,
    ordinal: u64,
    key: &str,
    uid: u64,
) -> Result<(), Error> {
    let (generation, b) = runtime.service.effective(key, uid)?;
    let p = runtime.service.package(key)?;
    let schema = p.schema.encode();
    let locks = runtime.service.locks(p);
    let names = locks.iter().map(String::as_str).collect::<Vec<_>>();
    let raw = rpc::read_only_vmo(&b)?;
    let config = [HandleRef { raw }];
    if ordinal == 1 {
        rpc::reply(
            Channel(client.channel),
            &UserPreferencesGetPreferencesResponse {
                status: Status::Ok,
                generation,
                config: &config,
                config_len: b.len() as u64,
                schema: &schema,
                locks: WireStringVector::from_slice(&names),
            },
        )
    } else {
        rpc::reply(
            Channel(client.channel),
            &UserPreferencesManageResponse {
                status: Status::Ok,
                generation,
                config: &config,
                config_len: b.len() as u64,
                schema: &schema,
                locks: WireStringVector::from_slice(&names),
            },
        )
    }
}
pub fn poll_users(runtime: &mut Runtime) {
    use user_manager_fidl::{
        FidlDecode, FidlEncode, HandleRef, UserManagerWatchUserEventsRequest,
        UserManagerWatchUserEventsResponse, UserStateWatcherOnUserStateChangedRequest, UserStatus,
    };
    if runtime.watcher.0 == 0 && runtime.usersd.0 != 0 {
        if let Ok((client, server)) = Channel::pair() {
            let mut b = [0; 32];
            let mut hs = [HandleRef { raw: 0 }; 1];
            if let Ok(e) = (UserManagerWatchUserEventsRequest {
                watcher: HandleRef { raw: client.0 },
            })
            .encode(&mut b, &mut hs)
            {
                let result = bexos_userspace::Rpc(runtime.usersd).call_raw(
                    8,
                    &b[..e.bytes],
                    &[client.0],
                    true,
                );
                if result.as_ref().is_ok_and(|m| {
                    UserManagerWatchUserEventsResponse::decode(&m.bytes, &[])
                        .is_ok_and(|r| r.status == UserStatus::Ok)
                }) {
                    runtime.watcher = server;
                } else {
                    let _ = Memory::close(server.0);
                }
            }
        }
    }
    if let Ok(m) = runtime.watcher.try_recv() {
        if m.bytes.len() >= 8 {
            if let Ok(q) = UserStateWatcherOnUserStateChangedRequest::decode(&m.bytes[8..], &[]) {
                if q.kind == user_manager_fidl::UserEventKind::Deleted {
                    runtime.delete_user(q.uid);
                } else if !q.unlocked {
                    runtime.lock(q.uid);
                } else {
                    runtime.service.locked.remove(&q.uid);
                }
            }
        }
    }
}
