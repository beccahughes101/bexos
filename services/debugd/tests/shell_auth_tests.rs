use bexos_debug_wire::*;
use bexos_debugd::{UserManager, shell_auth::authenticate};
struct Users {
    user: UserInfo,
    attempts: usize,
}
fn denied() -> DebugStatusResponse {
    DebugStatusResponse {
        status: -4,
        message: "denied".into(),
    }
}
impl UserManager for Users {
    async fn list_users(&mut self) -> Result<Vec<UserInfo>, DebugStatusResponse> {
        Ok(vec![self.user.clone()])
    }
    async fn get_user(&mut self, q: UserGetRequest) -> Result<UserInfo, DebugStatusResponse> {
        if q.uid == self.user.uid {
            Ok(self.user.clone())
        } else {
            Err(denied())
        }
    }
    async fn unlock_user(&mut self, q: UserUnlockRequest) -> DebugStatusResponse {
        self.attempts += 1;
        if q.uid == self.user.uid && q.password == "correct" {
            DebugStatusResponse::default()
        } else {
            denied()
        }
    }
    async fn create_user(&mut self, _: UserCreateRequest) -> DebugStatusResponse {
        denied()
    }
    async fn update_user(&mut self, _: UserUpdateRequest) -> DebugStatusResponse {
        denied()
    }
    async fn delete_user(&mut self, _: UserDeleteRequest) -> DebugStatusResponse {
        denied()
    }
    async fn lock_user(&mut self, _: UserLockRequest) -> DebugStatusResponse {
        denied()
    }
}
fn users() -> Users {
    Users {
        user: UserInfo {
            uid: 1000,
            name: "alice".into(),
            unlocked: true,
            ..Default::default()
        },
        attempts: 0,
    }
}
#[tokio::test]
async fn every_user_open_authenticates_even_when_unlocked() {
    let mut u = users();
    let mut q = ShellRequest {
        user: "alice".into(),
        password: "wrong".into(),
        ..Default::default()
    };
    assert!(authenticate(&mut u, &mut q).await.is_err());
    assert!(q.password.is_empty());
    q.password = "correct".into();
    assert_eq!(authenticate(&mut u, &mut q).await.unwrap().uid, 1000);
    assert_eq!(u.attempts, 2);
    assert!(q.password.is_empty());
}
#[tokio::test]
async fn rejects_disabled_missing_and_conflicting_identity() {
    let mut u = users();
    for mut q in [
        ShellRequest::default(),
        ShellRequest {
            system: true,
            uid: 1000,
            ..Default::default()
        },
        ShellRequest {
            uid: 1000,
            user: "alice".into(),
            ..Default::default()
        },
        ShellRequest {
            uid: 2000,
            ..Default::default()
        },
    ] {
        assert!(authenticate(&mut u, &mut q).await.is_err());
    }
    u.user.disabled = true;
    assert!(
        authenticate(
            &mut u,
            &mut ShellRequest {
                uid: 1000,
                password: "correct".into(),
                ..Default::default()
            }
        )
        .await
        .is_err()
    );
    assert_eq!(u.attempts, 0);
}
#[tokio::test]
async fn explicit_system_does_not_authenticate_a_user() {
    let mut u = users();
    let r = authenticate(
        &mut u,
        &mut ShellRequest {
            system: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(r.uid, 0);
    assert_eq!(u.attempts, 0);
}
