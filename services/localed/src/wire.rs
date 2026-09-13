use crate::{
    binding::Client,
    service::{Listener, Runtime, User},
    storage::READ_ONLY_RIGHTS,
};
use bexos_locale_settings::{Settings, encode};
use bexos_userspace::{Channel, Memory};
use locale_fidl::*;
fn reply<T: FidlEncode>(channel: u64, q: &T) -> Result<(), Status> {
    let (bytes, handles) = encode(q).map_err(|_| Status::InvalidArgs)?;
    Channel(channel)
        .send(&bytes, &handles)
        .map_err(|_| Status::Io)
}
fn ensure(runtime: &mut Runtime, uid: u64) -> Result<(), Status> {
    if uid == u64::MAX {
        return Err(Status::InvalidArgs);
    }
    if runtime.users.contains_key(&uid) {
        return Ok(());
    }
    if runtime.users.len() >= 256 {
        return Err(Status::Busy);
    }
    let (watch, remote) = Channel::pair().map_err(|_| Status::Io)?;
    let result = (|| {
        let (bytes, handles) = encode(&LocalePreferencesWatchRequest {
            uid,
            listener: HandleRef { raw: remote.0 },
        })
        .map_err(|_| {
            let _ = Memory::close(remote.0);
            Status::InvalidArgs
        })?;
        let mut request = 2u64.to_le_bytes().to_vec();
        request.extend(bytes);
        if runtime.preferences.send(&request, &handles).is_err() {
            let _ = Memory::close(remote.0);
            return Err(Status::Unavailable);
        }
        let message = runtime
            .preferences
            .recv_with_timeout(10)
            .map_err(|_| Status::Unavailable)?;
        let q = LocalePreferencesWatchResponse::decode(&message.bytes, &[])
            .map_err(|_| Status::InvalidArgs)?;
        if q.status != Status::Ok {
            return Err(q.status);
        }
        let settings =
            Settings::from_wire(&q.snapshot.settings).map_err(|_| Status::InvalidArgs)?;
        runtime.users.insert(
            uid,
            User {
                settings,
                generation: q.snapshot.generation,
                watch: watch.0,
            },
        );
        Ok(())
    })();
    if result.is_err() {
        let _ = Memory::close(watch.0);
    }
    // send transfers the remote endpoint on success. It must not be
    // closed after transfer: that slot may already have been reused.
    result
}
pub fn poll(runtime: &mut Runtime) {
    for client in runtime.clients.clone() {
        let message = match Channel(client.channel).try_recv() {
            Ok(m) => m,
            Err(kernel_fidl::Status::ErrPeerClosed) => {
                let _ = Memory::close(client.channel);
                runtime.clients.retain(|c| c.channel != client.channel);
                continue;
            }
            Err(_) => continue,
        };
        let ordinal = message
            .bytes
            .get(..8)
            .map(|b| u64::from_le_bytes(b.try_into().unwrap()))
            .unwrap_or(0);
        let hs = message
            .handles
            .iter()
            .map(|h| HandleRef { raw: *h })
            .collect::<Vec<_>>();
        let result = (|| {
            if !client.methods.contains(&ordinal)
                || message.handles.len() != usize::from(!client.startup && ordinal == 3)
            {
                return Err(Status::AccessDenied);
            }
            let bytes = message.bytes.get(8..).ok_or(Status::InvalidArgs)?;
            let uid = if client.startup {
                LocaleStartupResolveRequest::decode(bytes, &hs)
                    .map_err(|_| Status::InvalidArgs)?
                    .uid
            } else {
                client.uid
            };
            ensure(runtime, uid)?;
            let user = &runtime.users[&uid];
            let duplicate =
                || Memory::duplicate(runtime.data, READ_ONLY_RIGHTS).map_err(|_| Status::Io);
            if client.startup {
                let data = duplicate()?;
                let result = user.settings.with_wire(|settings| {
                    reply(
                        client.channel,
                        &LocaleStartupResolveResponse {
                            status: Status::Ok,
                            snapshot: LocaleSnapshot {
                                generation: user.generation,
                                settings,
                            },
                            data: &[HandleRef { raw: data }],
                            data_len: runtime.data_len,
                            data_generation: runtime.data_generation,
                        },
                    )
                });
                if result.is_err() {
                    let _ = Memory::close(data);
                }
                return result;
            }
            match ordinal {
                1 => {
                    LocaleProviderGetLocaleSettingsRequest::decode(bytes, &hs)
                        .map_err(|_| Status::InvalidArgs)?;
                    user.settings.with_wire(|settings| {
                        reply(
                            client.channel,
                            &LocaleProviderGetLocaleSettingsResponse {
                                status: Status::Ok,
                                snapshot: LocaleSnapshot {
                                    generation: user.generation,
                                    settings,
                                },
                            },
                        )
                    })
                }
                2 => {
                    LocaleProviderGetCldrDataVmoRequest::decode(bytes, &hs)
                        .map_err(|_| Status::InvalidArgs)?;
                    let data = duplicate()?;
                    let result = reply(
                        client.channel,
                        &LocaleProviderGetCldrDataVmoResponse {
                            status: Status::Ok,
                            data: &[HandleRef { raw: data }],
                            data_len: runtime.data_len,
                            data_generation: runtime.data_generation,
                        },
                    );
                    if result.is_err() {
                        let _ = Memory::close(data);
                    }
                    result
                }
                3 => {
                    let q = LocaleProviderWatchLocaleRequest::decode(bytes, &hs)
                        .map_err(|_| Status::InvalidArgs)?;
                    if runtime.listeners.len() >= 256 {
                        return Err(Status::Busy);
                    }
                    user.settings.with_wire(|settings| {
                        reply(
                            client.channel,
                            &LocaleProviderWatchLocaleResponse {
                                status: Status::Ok,
                                snapshot: LocaleSnapshot {
                                    generation: user.generation,
                                    settings,
                                },
                            },
                        )
                    })?;
                    runtime.listeners.push(Listener {
                        channel: q.listener.raw,
                        uid,
                        generation: user.generation,
                    });
                    Ok(())
                }
                _ => Err(Status::InvalidArgs),
            }
        })();
        if let Err(status) = result {
            for h in message.handles {
                let _ = Memory::close(h);
            }
            error(&client, ordinal, status);
        }
    }
}
fn error(client: &Client, ordinal: u64, status: Status) {
    Settings::default().with_wire(|settings| {
        let snapshot = LocaleSnapshot {
            generation: 0,
            settings,
        };
        if client.startup {
            let _ = reply(
                client.channel,
                &LocaleStartupResolveResponse {
                    status,
                    snapshot,
                    data: &[],
                    data_len: 0,
                    data_generation: 0,
                },
            );
        } else {
            match ordinal {
                1 => {
                    let _ = reply(
                        client.channel,
                        &LocaleProviderGetLocaleSettingsResponse { status, snapshot },
                    );
                }
                2 => {
                    let _ = reply(
                        client.channel,
                        &LocaleProviderGetCldrDataVmoResponse {
                            status,
                            data: &[],
                            data_len: 0,
                            data_generation: 0,
                        },
                    );
                }
                3 => {
                    let _ = reply(
                        client.channel,
                        &LocaleProviderWatchLocaleResponse { status, snapshot },
                    );
                }
                _ => {}
            }
        }
    });
}
pub fn poll_preferences(runtime: &mut Runtime) {
    let watches = runtime
        .users
        .iter()
        .map(|(uid, u)| (*uid, u.watch))
        .collect::<Vec<_>>();
    for (uid, watch) in watches {
        match Channel(watch).try_recv() {
            Ok(message) => {
                for h in &message.handles {
                    let _ = Memory::close(*h);
                }
                if message.bytes.get(..8) != Some(&1u64.to_le_bytes()) {
                    continue;
                }
                if let Ok(q) = LocaleUpdateListenerOnLocaleSettingsChangedRequest::decode(
                    &message.bytes[8..],
                    &[],
                ) {
                    if let Ok(settings) = Settings::from_wire(&q.snapshot.settings) {
                        runtime.update(uid, q.snapshot.generation, settings);
                    }
                }
            }
            Err(kernel_fidl::Status::ErrPeerClosed) => {
                let _ = Memory::close(watch);
                runtime.users.remove(&uid);
                runtime.listeners.retain(|o| {
                    if o.uid == uid {
                        let _ = Memory::close(o.channel);
                        false
                    } else {
                        true
                    }
                });
                // Retain authenticated public bindings: after unlock, an
                // existing app may register a new watch on the same endpoint.
                // Every request now goes through ensure() and prefsd's lock check.

            }
            Err(_) => {}
        }
    }
}
pub fn broadcast(runtime: &mut Runtime) {
    let mut closed = Vec::new();
    for listener in &mut runtime.listeners {
        match Channel(listener.channel).try_recv() {
            Ok(message) => {
                for h in message.handles {
                    let _ = Memory::close(h);
                }
                let _ = Memory::close(listener.channel);
                closed.push(listener.channel);
                continue;
            }
            Err(kernel_fidl::Status::ErrPeerClosed) => {
                let _ = Memory::close(listener.channel);
                closed.push(listener.channel);
                continue;
            }
            Err(_) => {}
        }
        let Some(user) = runtime.users.get(&listener.uid) else {
            continue;
        };
        if user.generation <= listener.generation {
            continue;
        }
        let result = user.settings.with_wire(|settings| {
            let (payload, _) = encode(&LocaleUpdateListenerOnLocaleSettingsChangedRequest {
                snapshot: LocaleSnapshot {
                    generation: user.generation,
                    settings,
                },
            })
            .map_err(|_| kernel_fidl::Status::ErrInvalidArgs)?;
            let mut bytes = 1u64.to_le_bytes().to_vec();
            bytes.extend(payload);
            Channel(listener.channel).send(&bytes, &[])
        });
        match result {
            Ok(()) => listener.generation = user.generation,
            Err(kernel_fidl::Status::ErrPeerClosed) => {
                let _ = Memory::close(listener.channel);
                closed.push(listener.channel);
            }
            Err(_) => {}
        }
    }
    runtime.listeners.retain(|l| !closed.contains(&l.channel));
}
