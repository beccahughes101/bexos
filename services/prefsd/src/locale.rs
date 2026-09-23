//! Private locale preference feed. Mutations still use the normal preferences API.
use crate::{
    binding::Client,
    runtime::Runtime,
    service::{Mutation, Service, ThemeObserver},
};
use alloc::{vec, vec::Vec};
use bexos_component_config::schema::Error;
use bexos_locale_settings::{PACKAGE_ID, Settings, encode};
use bexos_userspace::{Channel, Memory};
use locale_fidl::*;

pub fn validate_mutation(service: &Service, mutation: &Mutation) -> Result<(), Error> {
    match mutation {
        Mutation::User {
            package,
            uid,
            fingerprint,
            ..
        } if package == PACKAGE_ID => {
            for p in service
                .packages
                .values()
                .filter(|p| p.id == *package && p.schema.fingerprint() == *fingerprint)
            {
                let (_, bytes) = service.effective(&p.key, *uid)?;
                Settings::from_table(&bytes)?;
            }
        }
        Mutation::Operator { package, .. } => {
            let p = service.package(package)?;
            if p.id == PACKAGE_ID {
                let (_, bytes) = service.effective(&p.key, 0)?;
                Settings::from_table(&bytes)?;
            }
        }
        _ => {}
    }
    Ok(())
}
/// A resolving transaction may hold candidate values after an uncertain write.
/// Never expose them until storage has confirmed the durable commit.
pub fn snapshot(service: &Service, uid: u64) -> Result<(u64, Settings), Error> {
    use bexos_component_config::transaction::Phase;
    if service
        .pending
        .as_ref()
        .is_some_and(|p| !matches!(p.transaction.phase, Phase::Committed | Phase::Complete))
    {
        return Err(Error::Busy);
    }
    let key = &service.package(PACKAGE_ID)?.key;
    let (_, bytes) = service.effective(key, uid)?;
    Settings::from_table(&bytes)
}
fn reply<T: FidlEncode>(channel: u64, message: &T) -> Result<(), Error> {
    let (bytes, handles) = encode(message)?;
    Channel(channel)
        .send(&bytes, &handles)
        .map_err(|_| Error::Storage)
}
pub fn poll(runtime: &mut Runtime, client: &Client) {
    let message = match Channel(client.channel).try_recv() {
        Ok(m) => m,
        Err(kernel_fidl::Status::ErrPeerClosed) => {
            let _ = Memory::close(client.channel);
            runtime.clients.retain(|c| c.channel != client.channel);
            return;
        }
        Err(_) => return,
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
        if !client.methods.contains(&ordinal) || message.handles.len() != usize::from(ordinal == 2)
        {
            return Err(Error::AccessDenied);
        }
        let bytes = message.bytes.get(8..).ok_or(Error::Malformed)?;
        let (uid, listener) = match ordinal {
            1 => (
                LocalePreferencesGetRequest::decode(bytes, &hs)
                    .map_err(|_| Error::Malformed)?
                    .uid,
                None,
            ),
            2 => {
                let q = LocalePreferencesWatchRequest::decode(bytes, &hs)
                    .map_err(|_| Error::Malformed)?;
                (q.uid, Some(q.listener.raw))
            }
            _ => return Err(Error::AccessDenied),
        };
        if runtime.service.pending.is_some() {
            return Err(Error::Busy);
        }
        runtime.check_user(uid)?;
        let key = runtime.service.package(PACKAGE_ID)?.key.clone();
        runtime.load(&key, uid)?;
        let (generation, settings) = snapshot(&runtime.service, uid)?;
        if listener.is_some() && runtime.service.locale_observers.len() >= 256 {
            return Err(Error::Bounds);
        }
        settings.with_wire(|settings| {
            let snapshot = LocaleSnapshot {
                generation,
                settings,
            };
            if ordinal == 1 {
                reply(
                    client.channel,
                    &LocalePreferencesGetResponse {
                        status: Status::Ok,
                        snapshot,
                    },
                )
            } else {
                reply(
                    client.channel,
                    &LocalePreferencesWatchResponse {
                        status: Status::Ok,
                        snapshot,
                    },
                )
            }
        })?;
        if let Some(channel) = listener {
            runtime.service.locale_observers.push(ThemeObserver {
                channel,
                uid,
                generation,
            });
        }
        Ok::<(), Error>(())
    })();
    if let Err(error) = result {
        for h in message.handles {
            let _ = Memory::close(h);
        }
        let status = match error {
            Error::AccessDenied => Status::AccessDenied,
            Error::Busy => Status::Busy,
            Error::UnknownField => Status::NotFound,
            _ => Status::InvalidArgs,
        };
        Settings::default().with_wire(|settings| {
            let snapshot = LocaleSnapshot {
                generation: 0,
                settings,
            };
            if ordinal == 1 {
                let _ = reply(
                    client.channel,
                    &LocalePreferencesGetResponse { status, snapshot },
                );
            } else if ordinal == 2 {
                let _ = reply(
                    client.channel,
                    &LocalePreferencesWatchResponse { status, snapshot },
                );
            }
        });
    }
}
pub fn broadcast(service: &mut Service) {
    let mut closed = vec![];
    for index in 0..service.locale_observers.len() {
        let observer = service.locale_observers[index].clone();
        match Channel(observer.channel).try_recv() {
            Ok(message) => {
                for h in message.handles {
                    let _ = Memory::close(h);
                }
                let _ = Memory::close(observer.channel);
                closed.push(observer.channel);
                continue;
            }
            Err(kernel_fidl::Status::ErrPeerClosed) => {
                let _ = Memory::close(observer.channel);
                closed.push(observer.channel);
                continue;
            }
            Err(_) => {}
        }
        let Ok((generation, settings)) = snapshot(service, observer.uid) else {
            continue;
        };
        if observer.generation >= generation {
            continue;
        }
        let result = settings.with_wire(|settings| {
            let (payload, _) = encode(&LocaleUpdateListenerOnLocaleSettingsChangedRequest {
                snapshot: LocaleSnapshot {
                    generation,
                    settings,
                },
            })
            .map_err(|_| kernel_fidl::Status::ErrInvalidArgs)?;
            let mut bytes = 1u64.to_le_bytes().to_vec();
            bytes.extend(payload);
            Channel(observer.channel).send(&bytes, &[])
        });
        match result {
            Ok(()) => service.locale_observers[index].generation = generation,
            Err(kernel_fidl::Status::ErrPeerClosed) => {
                let _ = Memory::close(observer.channel);
                closed.push(observer.channel);
            }
            // Backpressure retains the old delivered generation for the next loop.
            Err(_) => {}
        }
    }
    service
        .locale_observers
        .retain(|o| !closed.contains(&o.channel));
}
