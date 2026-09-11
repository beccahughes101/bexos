use super::*;

impl<T: DebugTransport> DebugClient<T> {
    pub fn list_users(&mut self) -> Result<Vec<UserInfo>, DebugClientError> {
        let mut payload = Vec::new();
        encode_empty(&mut payload);
        let response = self.call(METHOD_LIST_USERS, payload)?;
        let response = decode_user_list_response(&response.payload)?;
        if response.status != 0 {
            return Err(DebugClientError::RemoteStatus(DebugStatusResponse {
                status: response.status,
                message: "user list failed".into(),
            }));
        }
        Ok(response.users)
    }

    pub fn get_user(&mut self, uid: u64) -> Result<UserInfo, DebugClientError> {
        let mut payload = Vec::new();
        encode_user_get_request(&UserGetRequest { uid }, &mut payload);
        let response = self.call(METHOD_GET_USER, payload)?;
        let response = decode_user_get_response(&response.payload)?;
        if response.status != 0 {
            return Err(DebugClientError::RemoteStatus(DebugStatusResponse {
                status: response.status,
                message: "user get failed".into(),
            }));
        }
        Ok(response.user)
    }

    pub fn create_user(
        &mut self,
        uid: u64,
        name: &str,
        display_name: &str,
        password: &str,
    ) -> Result<(), DebugClientError> {
        let mut payload = Vec::new();
        encode_user_create_request(
            &UserCreateRequest {
                uid,
                name: name.to_string(),
                display_name: display_name.to_string(),
                password: password.to_string(),
            },
            &mut payload,
        );
        self.status_call(METHOD_CREATE_USER, payload)
    }

    pub fn update_user(
        &mut self,
        uid: u64,
        name: &str,
        display_name: &str,
        disabled: bool,
    ) -> Result<(), DebugClientError> {
        let mut payload = Vec::new();
        encode_user_update_request(
            &UserUpdateRequest {
                uid,
                name: name.to_string(),
                display_name: display_name.to_string(),
                disabled,
                current_password: String::new(),
                new_password: String::new(),
            },
            &mut payload,
        );
        self.status_call(METHOD_UPDATE_USER, payload)
    }

    pub fn replace_user_password(
        &mut self,
        uid: u64,
        name: &str,
        display_name: &str,
        disabled: bool,
        current_password: &str,
        new_password: &str,
    ) -> Result<(), DebugClientError> {
        let mut payload = Vec::new();
        encode_user_update_request(
            &UserUpdateRequest {
                uid,
                name: name.to_string(),
                display_name: display_name.to_string(),
                disabled,
                current_password: current_password.to_string(),
                new_password: new_password.to_string(),
            },
            &mut payload,
        );
        self.status_call(METHOD_UPDATE_USER, payload)
    }

    pub fn delete_user(&mut self, uid: u64) -> Result<(), DebugClientError> {
        let mut payload = Vec::new();
        encode_user_delete_request(&UserDeleteRequest { uid }, &mut payload);
        self.status_call(METHOD_DELETE_USER, payload)
    }

    pub fn unlock_user(&mut self, uid: u64, password: &str) -> Result<(), DebugClientError> {
        let mut payload = Vec::new();
        encode_user_unlock_request(
            &UserUnlockRequest {
                uid,
                password: password.to_string(),
            },
            &mut payload,
        );
        self.status_call(METHOD_UNLOCK_USER, payload)
    }

    pub fn lock_user(&mut self, uid: u64) -> Result<(), DebugClientError> {
        let mut payload = Vec::new();
        encode_user_lock_request(&UserLockRequest { uid }, &mut payload);
        self.status_call(METHOD_LOCK_USER, payload)
    }
}
