use crate::{binding::Client, index::Query, runtime::Runtime};
use alloc::{vec, vec::Vec};
use bexos_userspace::{Channel, Memory};
use fonts_fidl::{
    FidlDecode, FidlEncode, FontProviderGetFallbackListRequest,
    FontProviderGetFallbackListResponse, FontProviderInstallUserFontRequest,
    FontProviderInstallUserFontResponse, FontProviderResolveFontRequest,
    FontProviderResolveFontResponse, FontStatus, HandleRef, WireVector,
};
use kernel_fidl::Status;

pub fn poll(runtime: &mut Runtime) -> bool {
    let mut changed = false;
    let clients = core::mem::take(&mut runtime.clients);
    for client in clients {
        match Channel(client.channel).try_recv() {
            Ok(message) => {
                changed = true;
                handle(runtime, &client, &message.bytes, &message.handles);
                runtime.clients.push(client);
            }
            Err(Status::ErrPeerClosed) => {
                changed = true;
                let _ = Memory::close(client.channel);
            }
            Err(_) => runtime.clients.push(client),
        }
    }
    changed
}

fn handle(runtime: &mut Runtime, client: &Client, bytes: &[u8], handles: &[u64]) {
    let Some((&ordinal, request)) = bytes.split_first_chunk::<8>() else {
        close_all(handles);
        return;
    };
    let ordinal = u64::from_le_bytes(ordinal);
    let refs = handles
        .iter()
        .map(|raw| HandleRef { raw: *raw })
        .collect::<Vec<_>>();
    if !client.methods.contains(&ordinal) {
        close_all(handles);
        invalid_reply(Channel(client.channel), ordinal);
        return;
    }
    match ordinal {
        1 => {
            close_all(handles);
            let response = match FontProviderResolveFontRequest::decode(request, &refs) {
                Ok(value) => {
                    let query = Query {
                        family: value.query.family_name.into(),
                        weight: value.query.weight,
                        style: value.query.style,
                        format: value.query.format_preference,
                    };
                    match runtime.resolve(client.uid, &query, value.allow_network_fetch) {
                        Ok(font) => {
                            let fonts = [font];
                            send_response(
                                Channel(client.channel),
                                &FontProviderResolveFontResponse {
                                    status: FontStatus::Ok,
                                    font: WireVector::from_slice(&fonts),
                                },
                            );
                            return;
                        }
                        Err(status) => resolve_error(status),
                    }
                }
                Err(_) => resolve_error(FontStatus::InvalidArgs),
            };
            send_response(Channel(client.channel), &response);
        }
        2 => {
            close_all(handles);
            let response = match FontProviderGetFallbackListRequest::decode(request, &refs) {
                Ok(value) => match runtime.fallbacks(client.uid, value.script) {
                    Ok(fonts) => {
                        let response = FontProviderGetFallbackListResponse {
                            status: FontStatus::Ok,
                            fonts: WireVector::from_slice(&fonts),
                        };
                        send_response(Channel(client.channel), &response);
                        return;
                    }
                    Err(status) => fallback_error(status),
                },
                Err(_) => fallback_error(FontStatus::InvalidArgs),
            };
            send_response(Channel(client.channel), &response);
        }
        3 => {
            if !client.install {
                close_all(handles);
                send_response(
                    Channel(client.channel),
                    &FontProviderInstallUserFontResponse {
                        status: FontStatus::AccessDenied,
                        assigned_id: 0,
                    },
                );
                return;
            }
            if handles.len() != 1 {
                close_all(handles);
                send_response(
                    Channel(client.channel),
                    &FontProviderInstallUserFontResponse {
                        status: FontStatus::InvalidArgs,
                        assigned_id: 0,
                    },
                );
                return;
            }
            let response = match FontProviderInstallUserFontRequest::decode(request, &refs) {
                Ok(value) if value.font_data.raw != 0 => {
                    match runtime.install(client.uid, value.font_data.raw, value.data_len) {
                        Ok(assigned_id) => FontProviderInstallUserFontResponse {
                            status: FontStatus::Ok,
                            assigned_id,
                        },
                        Err(status) => FontProviderInstallUserFontResponse {
                            status,
                            assigned_id: 0,
                        },
                    }
                }
                _ => {
                    close_all(handles);
                    FontProviderInstallUserFontResponse {
                        status: FontStatus::InvalidArgs,
                        assigned_id: 0,
                    }
                }
            };
            send_response(Channel(client.channel), &response);
        }
        _ => {
            close_all(handles);
            invalid_reply(Channel(client.channel), ordinal);
        }
    }
}

fn resolve_error(status: FontStatus) -> FontProviderResolveFontResponse<'static> {
    FontProviderResolveFontResponse {
        status,
        font: WireVector::from_slice(&[]),
    }
}

fn fallback_error(status: FontStatus) -> FontProviderGetFallbackListResponse<'static> {
    FontProviderGetFallbackListResponse {
        status,
        fonts: WireVector::from_slice(&[]),
    }
}

fn invalid_reply(channel: Channel, ordinal: u64) {
    match ordinal {
        1 => send_response(channel, &resolve_error(FontStatus::InvalidArgs)),
        2 => send_response(channel, &fallback_error(FontStatus::InvalidArgs)),
        3 => send_response(
            channel,
            &FontProviderInstallUserFontResponse {
                status: FontStatus::InvalidArgs,
                assigned_id: 0,
            },
        ),
        _ => {}
    }
}

fn send_response<T: FidlEncode>(channel: Channel, response: &T) {
    let mut bytes = vec![0; 65500];
    let mut handles = [HandleRef { raw: 0 }; 8];
    let Ok(encoded) = response.encode(&mut bytes, &mut handles) else {
        return;
    };
    let raw = handles[..encoded.handles]
        .iter()
        .map(|handle| handle.raw)
        .collect::<Vec<_>>();
    if channel.send(&bytes[..encoded.bytes], &raw).is_err() {
        close_all(&raw);
    }
}

fn close_all(handles: &[u64]) {
    for handle in handles {
        if *handle != 0 {
            let _ = Memory::close(*handle);
        }
    }
}

pub fn poll_users(runtime: &mut Runtime) -> bool {
    use user_manager_fidl::{
        FidlDecode, FidlEncode, HandleRef as UserHandleRef, UserEventKind,
        UserManagerWatchUserEventsRequest, UserManagerWatchUserEventsResponse,
        UserStateWatcherOnUserStateChangedRequest, UserStatus,
    };
    let mut changed = false;
    if runtime.watcher.0 == 0 && runtime.usersd.0 != 0 {
        if let Ok((client, server)) = Channel::pair() {
            let mut bytes = [0; 32];
            let mut handles = [UserHandleRef { raw: 0 }; 1];
            if let Ok(encoded) = (UserManagerWatchUserEventsRequest {
                watcher: UserHandleRef { raw: client.0 },
            })
            .encode(&mut bytes, &mut handles)
            {
                let response = bexos_userspace::Rpc(runtime.usersd).call_raw(
                    8,
                    &bytes[..encoded.bytes],
                    &[client.0],
                    true,
                );
                if response.as_ref().is_ok_and(|message| {
                    UserManagerWatchUserEventsResponse::decode(&message.bytes, &[])
                        .is_ok_and(|response| response.status == UserStatus::Ok)
                }) {
                    runtime.watcher = server;
                    changed = true;
                } else {
                    let _ = Memory::close(server.0);
                }
            }
        }
    }
    if let Ok(message) = runtime.watcher.try_recv() {
        if message.bytes.len() >= 8 {
            if let Ok(event) =
                UserStateWatcherOnUserStateChangedRequest::decode(&message.bytes[8..], &[])
            {
                if event.kind == UserEventKind::Deleted || !event.unlocked {
                    runtime.evict_user(event.uid);
                    changed = true;
                }
            }
        }
    }
    changed
}
