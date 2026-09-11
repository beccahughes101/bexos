use super::*;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BufferedUserManager {
    pub(super) users: Vec<UserInfo>,
}

impl BufferedUserManager {
    pub fn new() -> Self {
        Self::default()
    }
}

impl UserManager for BufferedUserManager {
    async fn list_users(&mut self) -> Result<Vec<UserInfo>, DebugStatusResponse> {
        Ok(self.users.clone())
    }

    async fn get_user(&mut self, request: UserGetRequest) -> Result<UserInfo, DebugStatusResponse> {
        self.users
            .iter()
            .find(|user| user.uid == request.uid)
            .cloned()
            .ok_or_else(|| debug_status(-1, "user not found"))
    }

    async fn create_user(&mut self, request: UserCreateRequest) -> DebugStatusResponse {
        if request.uid == 0 || request.name.is_empty() || request.password.is_empty() {
            return invalid_request_response("invalid user create request");
        }
        if self.users.iter().any(|user| user.uid == request.uid) {
            return debug_status(-2, "user already exists");
        }
        let display_name = if request.display_name.is_empty() {
            request.name.clone()
        } else {
            request.display_name
        };
        self.users.push(UserInfo {
            uid: request.uid,
            name: request.name,
            display_name,
            disabled: false,
            home_path: alloc::format!("data/users/{}", request.uid),
            unlocked: false,
        });
        self.users.sort_by_key(|user| user.uid);
        ok_response("user created")
    }

    async fn update_user(&mut self, request: UserUpdateRequest) -> DebugStatusResponse {
        let Some(user) = self.users.iter_mut().find(|user| user.uid == request.uid) else {
            return debug_status(-1, "user not found");
        };
        user.name = request.name;
        user.display_name = request.display_name;
        user.disabled = request.disabled;
        ok_response("user updated")
    }

    async fn delete_user(&mut self, request: UserDeleteRequest) -> DebugStatusResponse {
        let Some(index) = self.users.iter().position(|user| user.uid == request.uid) else {
            return debug_status(-1, "user not found");
        };
        self.users.remove(index);
        ok_response("user deleted")
    }

    async fn unlock_user(&mut self, request: UserUnlockRequest) -> DebugStatusResponse {
        let Some(user) = self.users.iter_mut().find(|user| user.uid == request.uid) else {
            return debug_status(-1, "user not found");
        };
        if request.password.is_empty() || user.disabled {
            return debug_status(-4, "user unlock denied");
        }
        user.unlocked = true;
        ok_response("user unlocked")
    }

    async fn lock_user(&mut self, request: UserLockRequest) -> DebugStatusResponse {
        let Some(user) = self.users.iter_mut().find(|user| user.uid == request.uid) else {
            return debug_status(-1, "user not found");
        };
        user.unlocked = false;
        ok_response("user locked")
    }
}
