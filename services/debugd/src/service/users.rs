use super::*;

pub trait UserManager {
    async fn list_users(&mut self) -> Result<Vec<UserInfo>, DebugStatusResponse>;
    async fn get_user(&mut self, request: UserGetRequest) -> Result<UserInfo, DebugStatusResponse>;
    async fn create_user(&mut self, request: UserCreateRequest) -> DebugStatusResponse;
    async fn update_user(&mut self, request: UserUpdateRequest) -> DebugStatusResponse;
    async fn delete_user(&mut self, request: UserDeleteRequest) -> DebugStatusResponse;
    async fn unlock_user(&mut self, request: UserUnlockRequest) -> DebugStatusResponse;
    async fn lock_user(&mut self, request: UserLockRequest) -> DebugStatusResponse;
}

pub struct UnsupportedUserManager;

impl UserManager for UnsupportedUserManager {
    async fn list_users(&mut self) -> Result<Vec<UserInfo>, DebugStatusResponse> {
        Err(unsupported_user_response())
    }

    async fn get_user(
        &mut self,
        _request: UserGetRequest,
    ) -> Result<UserInfo, DebugStatusResponse> {
        Err(unsupported_user_response())
    }

    async fn create_user(&mut self, _request: UserCreateRequest) -> DebugStatusResponse {
        unsupported_user_response()
    }

    async fn update_user(&mut self, _request: UserUpdateRequest) -> DebugStatusResponse {
        unsupported_user_response()
    }

    async fn delete_user(&mut self, _request: UserDeleteRequest) -> DebugStatusResponse {
        unsupported_user_response()
    }

    async fn unlock_user(&mut self, _request: UserUnlockRequest) -> DebugStatusResponse {
        unsupported_user_response()
    }

    async fn lock_user(&mut self, _request: UserLockRequest) -> DebugStatusResponse {
        unsupported_user_response()
    }
}
