//! Authentication is completed before any shell/provider handle is created.
use crate::UserManager;
use bexos_debug_wire::{
    DebugStatusResponse, ShellRequest, UserGetRequest, UserInfo, UserUnlockRequest,
};
fn failure(message: &str) -> DebugStatusResponse {
    DebugStatusResponse {
        status: -8,
        message: message.into(),
    }
}
pub async fn authenticate<U: UserManager>(
    users: &mut U,
    q: &mut ShellRequest,
) -> Result<UserInfo, DebugStatusResponse> {
    if q.system {
        if q.uid != 0 || !q.user.is_empty() || !q.password.is_empty() {
            return Err(failure("--system cannot be combined with user credentials"));
        }
        return Ok(UserInfo {
            uid: 0,
            name: "system".into(),
            display_name: "System".into(),
            home_path: "/data".into(),
            disabled: false,
            unlocked: true,
        });
    }
    if (q.uid == 0) == q.user.is_empty() {
        return Err(failure("select exactly one user name or nonzero UID"));
    }
    let user = if q.uid != 0 {
        users.get_user(UserGetRequest { uid: q.uid }).await?
    } else {
        let mut found = users
            .list_users()
            .await?
            .into_iter()
            .filter(|u| u.name == q.user);
        let u = found.next().ok_or_else(|| failure("user not found"))?;
        if found.next().is_some() {
            return Err(failure("ambiguous user name; select --uid"));
        }
        u
    };
    if user.uid == 0 || user.disabled {
        return Err(failure("user cannot log in"));
    }
    let result = users
        .unlock_user(UserUnlockRequest {
            uid: user.uid,
            password: core::mem::take(&mut q.password),
        })
        .await;
    if result.status != 0 {
        return Err(result);
    }
    Ok(user)
}
