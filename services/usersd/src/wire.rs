use alloc::vec;
use alloc::vec::Vec;
use bexos_userspace::service_binding::{BoundServiceEndpoint, ServiceBinding};
use bexos_userspace::{Channel, Rpc};
use user_manager_fidl::{
    FidlDecode, FidlEncode, HandleRef, UserEventKind, UserManagerCreateUserRequest,
    UserManagerCreateUserResponse, UserManagerDeleteUserRequest, UserManagerDeleteUserResponse,
    UserManagerGetHardwareAuthTokenRequest, UserManagerGetHardwareAuthTokenResponse,
    UserManagerGetUserRequest, UserManagerGetUserResponse, UserManagerListUsersResponse,
    UserManagerLockUserRequest, UserManagerLockUserResponse, UserManagerUnlockUserRequest,
    UserManagerUnlockUserResponse, UserManagerUpdateUserRequest, UserManagerUpdateUserResponse,
    UserManagerWatchUserEventsRequest, UserManagerWatchUserEventsResponse,
    UserStateWatcherOnUserStateChangedRequest, UserStateWatcherPublicClient, UserStatus,
    WireVector,
};

use crate::migration::Runtime;
use crate::service::{
    create_user, current_hardware_auth_token, delete_user, empty_user_info, is_unlocked,
    list_user_infos, lock_user, unlock_user, update_user, user_info, user_status,
};

pub fn poll_request(runtime: &mut Runtime) -> bool {
    let channel = runtime.control;
    let Ok(message) = channel.try_recv() else {
        return false;
    };
    if message.bytes == b"bexos.storage.ready" && message.handles.is_empty() {
        let status: i32 = if crate::runtime::attach_persistent_store(runtime).is_ok() {
            0
        } else {
            -1
        };
        let _ = channel.send(&status.to_le_bytes(), &[]);
        return true;
    }
    if let Some((endpoint, binding)) = message
        .handles
        .first()
        .copied()
        .zip(core::str::from_utf8(&message.bytes).ok())
        .and_then(|(endpoint, metadata)| {
            ServiceBinding::parse(metadata).map(|binding| (endpoint, binding))
        })
    {
        if binding.protocol_is("UserManager") {
            runtime.clients.push(BoundServiceEndpoint::new(
                Channel(endpoint),
                binding.method_ordinals,
            ));
        } else {
            let _ = bexos_userspace::Memory::close(endpoint);
        }
        return true;
    }
    dispatch_request(runtime, channel, message);
    true
}

pub fn poll_bound_requests(runtime: &mut Runtime) -> bool {
    let mut pending = Vec::new();
    runtime
        .clients
        .retain(|client| match client.channel.try_recv() {
            Ok(message) => {
                let ordinal = envelope(&message.bytes).0;
                if client.allows(ordinal) {
                    pending.push((client.channel, message));
                } else {
                    for handle in message.handles {
                        let _ = bexos_userspace::Memory::close(handle);
                    }
                }
                true
            }
            Err(kernel_fidl::Status::ErrTimedOut) => true,
            Err(kernel_fidl::Status::ErrPeerClosed) => false,
            Err(_) => true,
        });
    let changed = !pending.is_empty();
    for (channel, message) in pending {
        dispatch_request(runtime, channel, message);
    }
    changed
}

fn dispatch_request(runtime: &mut Runtime, channel: Channel, message: bexos_userspace::Message) {
    let (ordinal, req) = envelope(&message.bytes);
    let hs = refs(&message.handles);
    match ordinal {
        1 => {
            let (status, users) = match runtime.service.list_users() {
                Ok(users) => (UserStatus::Ok, users),
                Err(error) => (user_status(Some(error)), Vec::new()),
            };
            let infos = list_user_infos(&users, &runtime.unlocked);
            reply(
                channel,
                &UserManagerListUsersResponse {
                    status,
                    users: WireVector::from_slice(&infos),
                },
            );
        }
        2 => {
            let q = UserManagerGetUserRequest::decode(req, &hs).unwrap();
            let result = runtime.service.get_user(q.uid);
            let status = user_status(result.as_ref().err().copied());
            let user = result
                .as_ref()
                .map(|user| user_info(user, is_unlocked(&runtime.unlocked, q.uid)))
                .unwrap_or_else(|_| empty_user_info());
            reply(channel, &UserManagerGetUserResponse { status, user });
        }
        3 => {
            let q = UserManagerCreateUserRequest::decode(req, &hs).unwrap();
            let uid = q.uid;
            let status = create_user(runtime, q);
            if status == UserStatus::Ok {
                notify_user_watchers(runtime, UserEventKind::Created, uid, false);
            }
            reply(channel, &UserManagerCreateUserResponse { status });
        }
        4 => {
            let q = UserManagerUpdateUserRequest::decode(req, &hs).unwrap();
            let uid = q.uid;
            let status = update_user(runtime, q);
            if status == UserStatus::Ok {
                notify_user_watchers(
                    runtime,
                    UserEventKind::Updated,
                    uid,
                    is_unlocked(&runtime.unlocked, uid),
                );
            }
            reply(channel, &UserManagerUpdateUserResponse { status });
        }
        5 => {
            let q = UserManagerDeleteUserRequest::decode(req, &hs).unwrap();
            let uid = q.uid;
            let status = delete_user(runtime, q.uid);
            if status == UserStatus::Ok {
                notify_user_watchers(runtime, UserEventKind::Deleted, uid, false);
            }
            reply(channel, &UserManagerDeleteUserResponse { status });
        }
        6 => {
            let q = UserManagerUnlockUserRequest::decode(req, &hs).unwrap();
            let uid = q.uid;
            let status = unlock_user(runtime, q.uid, q.password);
            if status == UserStatus::Ok {
                notify_user_watchers(runtime, UserEventKind::Unlocked, uid, true);
            }
            reply(channel, &UserManagerUnlockUserResponse { status });
        }
        7 => {
            let q = UserManagerLockUserRequest::decode(req, &hs).unwrap();
            let uid = q.uid;
            let status = lock_user(runtime, q.uid);
            if status == UserStatus::Ok {
                notify_user_watchers(runtime, UserEventKind::Locked, uid, false);
            }
            reply(channel, &UserManagerLockUserResponse { status });
        }
        8 => {
            let status = match UserManagerWatchUserEventsRequest::decode(req, &hs) {
                Ok(q) if q.watcher.raw != 0 => {
                    runtime.watchers.push(q.watcher.raw);
                    UserStatus::Ok
                }
                _ => UserStatus::InvalidArgs,
            };
            reply(channel, &UserManagerWatchUserEventsResponse { status });
        }
        9 => {
            let q = UserManagerGetHardwareAuthTokenRequest::decode(req, &hs).unwrap();
            let (status, encoded_token, secure_timestamp_ms, expires_at_ms) =
                current_hardware_auth_token(runtime, q.uid);
            reply(
                channel,
                &UserManagerGetHardwareAuthTokenResponse {
                    status,
                    encoded_token: &encoded_token,
                    secure_timestamp_ms,
                    expires_at_ms,
                },
            );
        }
        _ => panic!("unknown usersd ordinal"),
    }
}

fn notify_user_watchers(runtime: &mut Runtime, kind: UserEventKind, uid: u64, unlocked: bool) {
    runtime.watchers.retain(|watcher| {
        let mut client = UserStateWatcherPublicClient::new(Rpc(Channel(*watcher)));
        let mut request_bytes = [0; 64];
        let mut request_handles = [HandleRef { raw: 0 }; 1];
        client
            .on_user_state_changed(
                &UserStateWatcherOnUserStateChangedRequest {
                    kind,
                    uid,
                    unlocked,
                },
                &mut request_bytes,
                &mut request_handles,
            )
            .is_ok()
    });
}

fn refs(handles: &[u64]) -> Vec<HandleRef> {
    handles
        .iter()
        .map(|handle| HandleRef { raw: *handle })
        .collect()
}

fn envelope(bytes: &[u8]) -> (u64, &[u8]) {
    assert!(bytes.len() >= 8);
    (
        u64::from_le_bytes(bytes[..8].try_into().unwrap()),
        &bytes[8..],
    )
}

fn reply<Q: FidlEncode>(channel: Channel, q: &Q) {
    let mut bytes = vec![0; 65500];
    let mut handles = [HandleRef { raw: 0 }; 16];
    let encoded = q.encode(&mut bytes, &mut handles).expect("usersd encode");
    channel
        .send(
            &bytes[..encoded.bytes],
            &handles[..encoded.handles]
                .iter()
                .map(|handle| handle.raw)
                .collect::<Vec<_>>(),
        )
        .expect("usersd reply");
}
