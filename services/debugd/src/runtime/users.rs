use super::*;

pub(crate) struct UserServiceManager {
    pub(crate) channel: Channel,
    pub(crate) awaiting_response: bool,
}

impl UserServiceManager {
    pub(crate) fn new(channel: Channel) -> Self {
        Self {
            channel,
            awaiting_response: false,
        }
    }

    pub(crate) fn call_raw<Q>(
        &mut self,
        ordinal: u64,
        request: &Q,
    ) -> Result<
        (
            alloc::vec::Vec<u8>,
            alloc::vec::Vec<user_manager::HandleRef>,
        ),
        user_manager::FidlWireError,
    >
    where
        Q: UserEncode,
    {
        // User mutations include nested durable VFS operations with a 300 s
        // budget. Reads share the ordered usersd channel and can queue behind
        // one of those mutations, so every request needs the same outer
        // allowance. This is independent of authentication-token lifetimes
        // and the secure replacement cutover deadlines.
        let timeout = 360;
        if self.awaiting_response {
            // Untagged FIDL replies must be drained before a new request can
            // use this channel, including after debugd itself is transplanted.
            let previous = self
                .channel
                .recv_with_timeout(timeout)
                .map_err(|_| user_manager::FidlWireError::Transport)?;
            for handle in previous.handles {
                let _ = Memory::close(handle);
            }
            self.awaiting_response = false;
        }
        let mut request_bytes = alloc::vec![0; 65500];
        let mut request_handles = [user_manager::HandleRef { raw: 0 }; 8];
        let encoded = request.encode(&mut request_bytes, &mut request_handles)?;
        let mut envelope = ordinal.to_le_bytes().to_vec();
        envelope.extend_from_slice(&request_bytes[..encoded.bytes]);
        self.channel
            .send(
                &envelope,
                &request_handles[..encoded.handles]
                    .iter()
                    .map(|h| h.raw)
                    .collect::<alloc::vec::Vec<_>>(),
            )
            .map_err(|_| user_manager::FidlWireError::Transport)?;
        self.awaiting_response = true;
        let message = self.channel.recv_with_timeout(timeout).map_err(|error| {
            bexos_userspace::log(&alloc::format!(
                "debugd: user RPC ordinal={ordinal} transport={error:?}; late reply fenced\n"
            ));
            user_manager::FidlWireError::Transport
        })?;
        self.awaiting_response = false;
        let response_handles = message
            .handles
            .iter()
            .map(|h| user_manager::HandleRef { raw: *h })
            .collect::<alloc::vec::Vec<_>>();
        Ok((message.bytes, response_handles))
    }
}

impl UserManager for UserServiceManager {
    async fn list_users(
        &mut self,
    ) -> Result<alloc::vec::Vec<bexos_debug_wire::UserInfo>, bexos_debug_wire::DebugStatusResponse>
    {
        let Ok((bytes, handles)) = self.call_raw(1, &user_manager::UserManagerListUsersRequest {})
        else {
            return Err(debug_status(-6, "user list transport"));
        };
        let response = user_manager::UserManagerListUsersResponse::decode(&bytes, &handles)
            .map_err(|_| debug_status(-6, "user list decode"))?;
        if response.status != user_manager::UserStatus::Ok {
            return Err(debug_status(response.status as i32, "user list failed"));
        }
        let mut users = alloc::vec::Vec::new();
        for index in 0..response.users.len() {
            users.push(debug_user_info(
                response
                    .users
                    .get(index)
                    .map_err(|_| debug_status(-6, "user info decode"))?,
            ));
        }
        Ok(users)
    }

    async fn get_user(
        &mut self,
        request: bexos_debug_wire::UserGetRequest,
    ) -> Result<bexos_debug_wire::UserInfo, bexos_debug_wire::DebugStatusResponse> {
        let Ok((bytes, handles)) = self.call_raw(
            2,
            &user_manager::UserManagerGetUserRequest { uid: request.uid },
        ) else {
            return Err(debug_status(-6, "user get transport"));
        };
        let response = user_manager::UserManagerGetUserResponse::decode(&bytes, &handles)
            .map_err(|_| debug_status(-6, "user get decode"))?;
        if response.status != user_manager::UserStatus::Ok {
            return Err(debug_status(response.status as i32, "user get failed"));
        }
        Ok(debug_user_info(response.user))
    }

    async fn create_user(
        &mut self,
        request: bexos_debug_wire::UserCreateRequest,
    ) -> bexos_debug_wire::DebugStatusResponse {
        let response = self.call_raw(
            3,
            &user_manager::UserManagerCreateUserRequest {
                uid: request.uid,
                name: &request.name,
                display_name: &request.display_name,
                password: &request.password,
            },
        );
        status_response(response, |bytes, handles| {
            user_manager::UserManagerCreateUserResponse::decode(bytes, handles)
                .map(|r| r.status as i32)
        })
    }

    async fn update_user(
        &mut self,
        request: bexos_debug_wire::UserUpdateRequest,
    ) -> bexos_debug_wire::DebugStatusResponse {
        let response = self.call_raw(
            4,
            &user_manager::UserManagerUpdateUserRequest {
                uid: request.uid,
                name: &request.name,
                display_name: &request.display_name,
                disabled: request.disabled,
                current_password: &request.current_password,
                new_password: &request.new_password,
            },
        );
        status_response(response, |bytes, handles| {
            user_manager::UserManagerUpdateUserResponse::decode(bytes, handles)
                .map(|r| r.status as i32)
        })
    }

    async fn delete_user(
        &mut self,
        request: bexos_debug_wire::UserDeleteRequest,
    ) -> bexos_debug_wire::DebugStatusResponse {
        let response = self.call_raw(
            5,
            &user_manager::UserManagerDeleteUserRequest { uid: request.uid },
        );
        status_response(response, |bytes, handles| {
            user_manager::UserManagerDeleteUserResponse::decode(bytes, handles)
                .map(|r| r.status as i32)
        })
    }

    async fn unlock_user(
        &mut self,
        request: bexos_debug_wire::UserUnlockRequest,
    ) -> bexos_debug_wire::DebugStatusResponse {
        let response = self.call_raw(
            6,
            &user_manager::UserManagerUnlockUserRequest {
                uid: request.uid,
                password: &request.password,
            },
        );
        status_response(response, |bytes, handles| {
            user_manager::UserManagerUnlockUserResponse::decode(bytes, handles)
                .map(|r| r.status as i32)
        })
    }

    async fn lock_user(
        &mut self,
        request: bexos_debug_wire::UserLockRequest,
    ) -> bexos_debug_wire::DebugStatusResponse {
        let response = self.call_raw(
            7,
            &user_manager::UserManagerLockUserRequest { uid: request.uid },
        );
        status_response(response, |bytes, handles| {
            user_manager::UserManagerLockUserResponse::decode(bytes, handles)
                .map(|r| r.status as i32)
        })
    }
}
