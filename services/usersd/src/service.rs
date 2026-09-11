use alloc::string::ToString;
use alloc::vec::Vec;
use bexos_trusty_client::users::{HardwareAuthToken, zeroize_ukek};
#[cfg(feature = "persistent")]
use bexos_user_store::persistent::UserStoreDb;
use bexos_user_store::{
    AuthenticatorKind, CreateUserRequest, MemoryUserStore, UpdateUserRequest, User, UserState,
    UserStoreError,
};
use bexos_userspace::{Memory, vfs};
use user_manager_fidl::{
    UserInfo, UserManagerCreateUserRequest, UserManagerUpdateUserRequest, UserStatus,
};

use crate::auth::UserAuthProvider;
use crate::migration::Runtime;
use crate::token_cache::{AuthStateChange, invalidate_for_change, retain_usable};

pub struct UsersdService {
    store: Store,
}

enum Store {
    Pending,
    Memory(MemoryUserStore),
    #[cfg(feature = "persistent")]
    Persistent(UserStoreDb),
}

impl UsersdService {
    pub fn awaiting_storage() -> Self {
        Self {
            store: Store::Pending,
        }
    }

    pub fn storage_pending(&self) -> bool {
        matches!(self.store, Store::Pending)
    }
    pub fn new() -> Self {
        Self {
            store: Store::Memory(MemoryUserStore::new()),
        }
    }

    #[cfg(feature = "persistent")]
    pub fn persistent(store: UserStoreDb) -> Self {
        Self {
            store: Store::Persistent(store),
        }
    }

    pub fn list_users(&self) -> Result<Vec<User>, UserStoreError> {
        match &self.store {
            Store::Pending => Err(UserStoreError::Storage),
            Store::Memory(store) => Ok(store.list_users().to_vec()),
            #[cfg(feature = "persistent")]
            Store::Persistent(store) => store.list_users(),
        }
    }

    pub fn get_user(&self, uid: u64) -> Result<User, UserStoreError> {
        match &self.store {
            Store::Pending => Err(UserStoreError::Storage),
            Store::Memory(store) => store.get_user(uid).cloned(),
            #[cfg(feature = "persistent")]
            Store::Persistent(store) => store.get_user(uid),
        }
    }

    fn create_user(&mut self, request: CreateUserRequest) -> Result<User, UserStoreError> {
        match &mut self.store {
            Store::Pending => Err(UserStoreError::Storage),
            Store::Memory(store) => store.create_user(request),
            #[cfg(feature = "persistent")]
            Store::Persistent(store) => store.create_user(request),
        }
    }

    fn update_user(&mut self, request: UpdateUserRequest) -> Result<User, UserStoreError> {
        match &mut self.store {
            Store::Pending => Err(UserStoreError::Storage),
            Store::Memory(store) => store.update_user(request),
            #[cfg(feature = "persistent")]
            Store::Persistent(store) => store.update_user(request),
        }
    }

    fn delete_user(&mut self, uid: u64) -> Result<User, UserStoreError> {
        match &mut self.store {
            Store::Pending => Err(UserStoreError::Storage),
            Store::Memory(store) => store.delete_user(uid),
            #[cfg(feature = "persistent")]
            Store::Persistent(store) => store.delete_user(uid),
        }
    }
}

impl Default for UsersdService {
    fn default() -> Self {
        Self::new()
    }
}

pub fn list_user_infos<'a>(users: &'a [User], unlocked: &[u64]) -> Vec<UserInfo<'a>> {
    users
        .iter()
        .map(|user| user_info(user, is_unlocked(unlocked, user.record.uid)))
        .collect()
}

pub fn create_user(state: &mut Runtime, q: UserManagerCreateUserRequest<'_>) -> UserStatus {
    if state.service.storage_pending() {
        return UserStatus::Storage;
    }
    let enrollment = match state.auth.enroll_password(q.uid, q.password) {
        Ok(enrollment) => enrollment,
        Err(status) => return status,
    };
    state.generation = state.generation.saturating_add(1);
    let created = state.service.create_user(CreateUserRequest {
        uid: q.uid,
        name: q.name.to_string(),
        display_name: q.display_name.to_string(),
        secure_user_id: enrollment.secure_user_id,
        gatekeeper_password_handle: enrollment.password_handle.clone(),
        auth_bound_hmac_key_blob: enrollment.hmac_key_blob.clone(),
        generation: state.generation,
    });
    match created {
        Ok(_) => match vfs::create_user_filesystem(state.vfsd, q.uid) {
            Ok(()) => {
                let status = unlock_user(state, q.uid, q.password);
                let _ = lock_user(state, q.uid);
                if status == UserStatus::Ok {
                    UserStatus::Ok
                } else {
                    rollback_created_user(state, q.uid, &enrollment);
                    status
                }
            }
            Err(error) => {
                bexos_userspace::log(&alloc::format!(
                    "usersd: create user filesystem failed: {error:?}\n"
                ));
                rollback_created_user(state, q.uid, &enrollment);
                UserStatus::Storage
            }
        },
        Err(error) => {
            rollback_enrollment(state, q.uid, &enrollment);
            user_status(Some(error))
        }
    }
}

pub fn update_user(state: &mut Runtime, q: UserManagerUpdateUserRequest<'_>) -> UserStatus {
    let replacing_password = !q.current_password.is_empty() || !q.new_password.is_empty();
    if replacing_password && (q.current_password.is_empty() || q.new_password.is_empty()) {
        return UserStatus::InvalidArgs;
    }
    let mut replacement_rollback = None;
    let replacement_handle = if replacing_password {
        let user = match state.service.get_user(q.uid) {
            Ok(user) => user,
            Err(error) => return user_status(Some(error)),
        };
        if user.record.state == UserState::Disabled {
            return UserStatus::AccessDenied;
        }
        let Some(authenticator) = password_authenticator(&user) else {
            return UserStatus::BadState;
        };
        match state.auth.replace_password(
            q.uid,
            authenticator.secure_user_id,
            &authenticator.gatekeeper_password_handle,
            q.current_password,
            q.new_password,
        ) {
            Ok(handle) => {
                replacement_rollback = Some((authenticator.secure_user_id, handle.clone()));
                Some(handle)
            }
            Err(status) => return status,
        }
    } else {
        None
    };
    state.generation = state.generation.saturating_add(1);
    let result = state.service.update_user(UpdateUserRequest {
        uid: q.uid,
        name: q.name.to_string(),
        display_name: q.display_name.to_string(),
        disabled: q.disabled,
        gatekeeper_password_handle: replacement_handle,
        generation: state.generation,
    });
    if result.is_err()
        && let Some((secure_user_id, replacement_handle)) = replacement_rollback
    {
        // Gatekeeper reenrollment precedes the durable metadata update. If
        // that update fails, restore the prior credential while both password
        // values are still available so the account is not stranded on an
        // unpersisted handle.
        let _ = state.auth.replace_password(
            q.uid,
            secure_user_id,
            &replacement_handle,
            q.new_password,
            q.current_password,
        );
    }
    if result.is_ok() && replacing_password {
        invalidate_for_change(
            &mut state.auth_tokens,
            &mut state.unlocked,
            q.uid,
            AuthStateChange::PasswordReplacement,
        );
    }
    if result.is_ok() && q.disabled {
        invalidate_for_change(
            &mut state.auth_tokens,
            &mut state.unlocked,
            q.uid,
            AuthStateChange::Disable,
        );
        match vfs::lock_user_filesystem(state.vfsd, q.uid) {
            Ok(()) | Err(fs_fidl::FsStatus::Locked) | Err(fs_fidl::FsStatus::NotFound) => {}
            Err(_) => return UserStatus::Storage,
        }
    }
    user_status(result.err())
}

pub fn delete_user(state: &mut Runtime, uid: u64) -> UserStatus {
    let user = match state.service.get_user(uid) {
        Ok(user) => user,
        Err(_) => return UserStatus::NotFound,
    };
    let authenticator = password_authenticator(&user).cloned();
    match vfs::lock_user_filesystem(state.vfsd, uid) {
        Ok(()) | Err(fs_fidl::FsStatus::NotFound) | Err(fs_fidl::FsStatus::Locked) => {}
        Err(_) => return UserStatus::Storage,
    }
    if let Some(authenticator) = &authenticator {
        if let Err(status) = state.auth.delete_user(
            uid,
            authenticator.secure_user_id,
            &authenticator.auth_bound_hmac_key_blob,
        ) {
            return status;
        }
    }
    match vfs::remove_user_filesystem(state.vfsd, uid) {
        Ok(()) | Err(fs_fidl::FsStatus::NotFound) => {}
        Err(_) => return UserStatus::Storage,
    }
    match state.service.delete_user(uid) {
        Ok(_) => {
            invalidate_for_change(
                &mut state.auth_tokens,
                &mut state.unlocked,
                uid,
                AuthStateChange::Delete,
            );
            UserStatus::Ok
        }
        Err(error) => user_status(Some(error)),
    }
}

pub fn unlock_user(state: &mut Runtime, uid: u64, password: &str) -> UserStatus {
    let user = match state.service.get_user(uid) {
        Ok(user) => user,
        Err(error) => return user_status(Some(error)),
    };
    if user.record.state == UserState::Disabled {
        return UserStatus::AccessDenied;
    }
    let Some(authenticator) = password_authenticator(&user) else {
        return UserStatus::BadState;
    };
    let token = match state.auth.verify_password(
        uid,
        authenticator.secure_user_id,
        &authenticator.gatekeeper_password_handle,
        password,
    ) {
        Ok(token) => token,
        Err(status) => {
            bexos_userspace::log(&alloc::format!(
                "usersd: password verification failed uid={uid} status={status:?}\n"
            ));
            return status;
        }
    };
    let mut ukek =
        match state
            .auth
            .derive_ukek(uid, &authenticator.auth_bound_hmac_key_blob, &token)
        {
            Ok(ukek) => ukek,
            Err(status) => {
                bexos_userspace::log(&alloc::format!(
                    "usersd: user key derivation failed uid={uid} status={status:?}\n"
                ));
                return status;
            }
        };
    let ukek_vmo = match Memory::from_bytes(&ukek) {
        Ok(handle) => handle,
        Err(_) => {
            zeroize_ukek(&mut ukek);
            return UserStatus::Storage;
        }
    };
    let already_unlocked = is_unlocked(&state.unlocked, uid);
    let unlock = if already_unlocked {
        Ok(())
    } else {
        vfs::unlock_user_filesystem(state.vfsd, uid, ukek_vmo).map_err(|error| {
            bexos_userspace::log(&alloc::format!(
                "usersd: user filesystem unlock failed uid={uid} error={error:?}\n"
            ));
            UserStatus::Storage
        })
    };
    zeroize_ukek(&mut ukek);
    let _ = Memory::close(ukek_vmo);
    if unlock.is_err() {
        return UserStatus::Storage;
    }
    if !is_unlocked(&state.unlocked, uid) {
        state.unlocked.push(uid);
    }
    cache_auth_token(state, token);
    UserStatus::Ok
}

pub fn lock_user(state: &mut Runtime, uid: u64) -> UserStatus {
    if state.service.get_user(uid).is_err() {
        return UserStatus::NotFound;
    }
    match vfs::lock_user_filesystem(state.vfsd, uid) {
        Ok(()) | Err(fs_fidl::FsStatus::Locked) => {}
        Err(_) => return UserStatus::Storage,
    }
    invalidate_for_change(
        &mut state.auth_tokens,
        &mut state.unlocked,
        uid,
        AuthStateChange::Lock,
    );
    UserStatus::Ok
}

pub fn current_hardware_auth_token(
    state: &mut Runtime,
    uid: u64,
) -> (UserStatus, Vec<u8>, u64, u64) {
    let now_ms = state.auth.now_ms();
    retain_usable(&mut state.auth_tokens, &state.unlocked, now_ms);
    match state
        .auth_tokens
        .iter()
        .find(|token| token.uid == uid && token.is_valid_at(now_ms))
    {
        Some(token) => (
            UserStatus::Ok,
            token.encoded.clone(),
            token.secure_timestamp_ms,
            token.expires_at_ms,
        ),
        None => (UserStatus::Locked, Vec::new(), 0, 0),
    }
}

pub fn user_info(user: &User, unlocked: bool) -> UserInfo<'_> {
    UserInfo {
        uid: user.record.uid,
        name: &user.record.name,
        display_name: &user.record.display_name,
        disabled: user.record.state == UserState::Disabled,
        home_path: &user.filesystem.root_path,
        unlocked,
    }
}

pub fn empty_user_info() -> UserInfo<'static> {
    UserInfo {
        uid: 0,
        name: "",
        display_name: "",
        disabled: false,
        home_path: "",
        unlocked: false,
    }
}

pub fn is_unlocked(unlocked: &[u64], uid: u64) -> bool {
    unlocked.iter().any(|candidate| *candidate == uid)
}

pub fn user_status(error: Option<UserStoreError>) -> UserStatus {
    match error {
        None => UserStatus::Ok,
        Some(UserStoreError::AlreadyExists) => UserStatus::AlreadyExists,
        Some(UserStoreError::NotFound) => UserStatus::NotFound,
        Some(UserStoreError::AccessDenied) => UserStatus::AccessDenied,
        Some(UserStoreError::Storage) => UserStatus::Storage,
        Some(UserStoreError::InvalidUid | UserStoreError::InvalidName) => UserStatus::InvalidArgs,
        Some(UserStoreError::MissingPasswordAuthenticator | UserStoreError::CorruptRecord) => {
            UserStatus::BadState
        }
    }
}

fn password_authenticator(user: &User) -> Option<&bexos_user_store::PasswordAuthenticator> {
    user.authenticators.iter().find_map(|authenticator| {
        if authenticator.kind == AuthenticatorKind::Password {
            authenticator.password.as_ref()
        } else {
            None
        }
    })
}

fn cache_auth_token(state: &mut Runtime, token: HardwareAuthToken) {
    state
        .auth_tokens
        .retain(|candidate| candidate.uid != token.uid);
    state.auth_tokens.push(token);
}

fn invalidate_auth_token(state: &mut Runtime, uid: u64) {
    state.auth_tokens.retain(|candidate| candidate.uid != uid);
}

fn rollback_created_user(state: &mut Runtime, uid: u64, enrollment: &crate::auth::Enrollment) {
    let _ = vfs::remove_user_filesystem(state.vfsd, uid);
    let _ = state.service.delete_user(uid);
    invalidate_auth_token(state, uid);
    rollback_enrollment(state, uid, enrollment);
}

fn rollback_enrollment(state: &mut Runtime, uid: u64, enrollment: &crate::auth::Enrollment) {
    let _ = state
        .auth
        .delete_user(uid, enrollment.secure_user_id, &enrollment.hmac_key_blob);
}
