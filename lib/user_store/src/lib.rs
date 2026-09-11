#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UserState {
    Ok,
    Disabled,
}

impl UserState {
    fn to_u64(self) -> u64 {
        match self {
            Self::Ok => 1,
            Self::Disabled => 2,
        }
    }

    fn from_u64(value: u64) -> Result<Self, UserStoreError> {
        match value {
            1 => Ok(Self::Ok),
            2 => Ok(Self::Disabled),
            _ => Err(UserStoreError::CorruptRecord),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthenticatorKind {
    Password,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UserRecord {
    pub uid: u64,
    pub name: String,
    pub display_name: String,
    pub state: UserState,
    pub created_generation: u64,
    pub updated_generation: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UserFilesystem {
    pub root_path: String,
    pub encrypted: bool,
    pub unlocked: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct User {
    pub record: UserRecord,
    pub authenticators: Vec<Authenticator>,
    pub filesystem: UserFilesystem,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Authenticator {
    pub authenticator_id: u64,
    pub kind: AuthenticatorKind,
    pub label: String,
    pub password: Option<PasswordAuthenticator>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PasswordAuthenticator {
    pub secure_user_id: u64,
    pub gatekeeper_password_handle: Vec<u8>,
    pub auth_bound_hmac_key_blob: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreateUserRequest {
    pub uid: u64,
    pub name: String,
    pub display_name: String,
    pub secure_user_id: u64,
    pub gatekeeper_password_handle: Vec<u8>,
    pub auth_bound_hmac_key_blob: Vec<u8>,
    pub generation: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpdateUserRequest {
    pub uid: u64,
    pub name: String,
    pub display_name: String,
    pub disabled: bool,
    pub gatekeeper_password_handle: Option<Vec<u8>>,
    pub generation: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UserStoreError {
    InvalidUid,
    InvalidName,
    MissingPasswordAuthenticator,
    AlreadyExists,
    NotFound,
    AccessDenied,
    CorruptRecord,
    Storage,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MemoryUserStore {
    users: Vec<User>,
}

impl MemoryUserStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn list_users(&self) -> &[User] {
        &self.users
    }

    pub fn get_user(&self, uid: u64) -> Result<&User, UserStoreError> {
        self.find(uid)
            .map(|index| &self.users[index])
            .ok_or(UserStoreError::NotFound)
    }

    pub fn create_user(&mut self, request: CreateUserRequest) -> Result<User, UserStoreError> {
        validate_uid(request.uid)?;
        validate_name(&request.name)?;
        if self.find(request.uid).is_some() {
            return Err(UserStoreError::AlreadyExists);
        }
        let display_name = if request.display_name.is_empty() {
            request.name.clone()
        } else {
            validate_display_name(&request.display_name)?;
            request.display_name
        };
        let user = User {
            record: UserRecord {
                uid: request.uid,
                name: request.name,
                display_name,
                state: UserState::Ok,
                created_generation: request.generation,
                updated_generation: request.generation,
            },
            authenticators: alloc::vec![Authenticator {
                authenticator_id: 1,
                kind: AuthenticatorKind::Password,
                label: "password".into(),
                password: Some(PasswordAuthenticator {
                    secure_user_id: request.secure_user_id,
                    gatekeeper_password_handle: request.gatekeeper_password_handle,
                    auth_bound_hmac_key_blob: request.auth_bound_hmac_key_blob,
                }),
            }],
            filesystem: UserFilesystem {
                root_path: user_home_path(request.uid),
                encrypted: true,
                unlocked: false,
            },
        };
        validate_user(&user)?;
        self.users.push(user.clone());
        self.users.sort_by_key(|user| user.record.uid);
        Ok(user)
    }

    pub fn update_user(&mut self, request: UpdateUserRequest) -> Result<User, UserStoreError> {
        validate_uid(request.uid)?;
        validate_name(&request.name)?;
        validate_display_name(&request.display_name)?;
        let index = self.find(request.uid).ok_or(UserStoreError::NotFound)?;
        self.users[index].record.name = request.name;
        self.users[index].record.display_name = request.display_name;
        self.users[index].record.state = if request.disabled {
            UserState::Disabled
        } else {
            UserState::Ok
        };
        if let Some(handle) = request.gatekeeper_password_handle {
            let password = self.users[index]
                .authenticators
                .iter_mut()
                .find_map(|authenticator| authenticator.password.as_mut())
                .ok_or(UserStoreError::MissingPasswordAuthenticator)?;
            password.gatekeeper_password_handle = handle;
        }
        self.users[index].record.updated_generation = request.generation;
        validate_user(&self.users[index])?;
        Ok(self.users[index].clone())
    }

    pub fn delete_user(&mut self, uid: u64) -> Result<User, UserStoreError> {
        validate_uid(uid)?;
        let index = self.find(uid).ok_or(UserStoreError::NotFound)?;
        Ok(self.users.remove(index))
    }

    pub fn set_unlocked(&mut self, uid: u64, unlocked: bool) -> Result<(), UserStoreError> {
        let index = self.find(uid).ok_or(UserStoreError::NotFound)?;
        self.users[index].filesystem.unlocked = unlocked;
        Ok(())
    }

    pub fn import_decoded(&mut self, user: User) -> Result<(), UserStoreError> {
        validate_user(&user)?;
        if self.find(user.record.uid).is_some() {
            return Err(UserStoreError::AlreadyExists);
        }
        self.users.push(user);
        self.users.sort_by_key(|user| user.record.uid);
        Ok(())
    }

    fn find(&self, uid: u64) -> Option<usize> {
        self.users.iter().position(|user| user.record.uid == uid)
    }
}

pub fn user_home_path(uid: u64) -> String {
    alloc::format!("data/users/{uid}")
}

pub fn encode_user(user: &User) -> Vec<u8> {
    let mut out = Vec::new();
    put_message(&mut out, 1, &encode_user_record(&user.record));
    for authenticator in &user.authenticators {
        put_message(&mut out, 2, &encode_authenticator(authenticator));
    }
    put_message(&mut out, 3, &encode_filesystem(&user.filesystem));
    out
}

pub fn decode_user(bytes: &[u8]) -> Result<User, UserStoreError> {
    let mut record = None;
    let mut authenticators = Vec::new();
    let mut filesystem = None;
    read_fields(bytes, |field| {
        match field.number {
            1 => record = Some(decode_user_record(field.bytes()?)?),
            2 => authenticators.push(decode_authenticator(field.bytes()?)?),
            3 => filesystem = Some(decode_filesystem(field.bytes()?)?),
            _ => {}
        }
        Ok(())
    })?;
    let user = User {
        record: record.ok_or(UserStoreError::CorruptRecord)?,
        authenticators,
        filesystem: filesystem.ok_or(UserStoreError::CorruptRecord)?,
    };
    validate_user(&user)?;
    Ok(user)
}

fn encode_user_record(record: &UserRecord) -> Vec<u8> {
    let mut out = Vec::new();
    put_varint(&mut out, 1, record.uid as u64);
    put_string(&mut out, 2, &record.name);
    put_string(&mut out, 3, &record.display_name);
    put_varint(&mut out, 4, record.state.to_u64());
    put_varint(&mut out, 5, record.created_generation);
    put_varint(&mut out, 6, record.updated_generation);
    out
}

fn decode_user_record(bytes: &[u8]) -> Result<UserRecord, UserStoreError> {
    let mut record = UserRecord {
        uid: 0,
        name: String::new(),
        display_name: String::new(),
        state: UserState::Ok,
        created_generation: 0,
        updated_generation: 0,
    };
    read_fields(bytes, |field| {
        match field.number {
            1 => record.uid = field.varint()?,
            2 => record.name = field.string()?,
            3 => record.display_name = field.string()?,
            4 => record.state = UserState::from_u64(field.varint()?)?,
            5 => record.created_generation = field.varint()?,
            6 => record.updated_generation = field.varint()?,
            _ => {}
        }
        Ok(())
    })?;
    Ok(record)
}

fn encode_filesystem(filesystem: &UserFilesystem) -> Vec<u8> {
    let mut out = Vec::new();
    put_string(&mut out, 1, &filesystem.root_path);
    put_varint(&mut out, 2, filesystem.encrypted as u64);
    put_varint(&mut out, 3, filesystem.unlocked as u64);
    out
}

fn decode_filesystem(bytes: &[u8]) -> Result<UserFilesystem, UserStoreError> {
    let mut filesystem = UserFilesystem {
        root_path: String::new(),
        encrypted: false,
        unlocked: false,
    };
    read_fields(bytes, |field| {
        match field.number {
            1 => filesystem.root_path = field.string()?,
            2 => filesystem.encrypted = field.varint()? != 0,
            3 => filesystem.unlocked = field.varint()? != 0,
            _ => {}
        }
        Ok(())
    })?;
    Ok(filesystem)
}

fn encode_authenticator(authenticator: &Authenticator) -> Vec<u8> {
    let mut out = Vec::new();
    put_varint(&mut out, 1, authenticator.authenticator_id);
    put_varint(
        &mut out,
        2,
        match authenticator.kind {
            AuthenticatorKind::Password => 1,
        },
    );
    put_string(&mut out, 3, &authenticator.label);
    if let Some(password) = &authenticator.password {
        put_message(&mut out, 4, &encode_password_authenticator(password));
    }
    out
}

fn decode_authenticator(bytes: &[u8]) -> Result<Authenticator, UserStoreError> {
    let mut authenticator = Authenticator {
        authenticator_id: 0,
        kind: AuthenticatorKind::Password,
        label: String::new(),
        password: None,
    };
    read_fields(bytes, |field| {
        match field.number {
            1 => authenticator.authenticator_id = field.varint()?,
            2 => {
                authenticator.kind = match field.varint()? {
                    1 => AuthenticatorKind::Password,
                    _ => return Err(UserStoreError::CorruptRecord),
                }
            }
            3 => authenticator.label = field.string()?,
            4 => authenticator.password = Some(decode_password_authenticator(field.bytes()?)?),
            _ => {}
        }
        Ok(())
    })?;
    Ok(authenticator)
}

fn encode_password_authenticator(password: &PasswordAuthenticator) -> Vec<u8> {
    let mut out = Vec::new();
    put_varint(&mut out, 1, password.secure_user_id);
    put_bytes(&mut out, 2, &password.gatekeeper_password_handle);
    put_bytes(&mut out, 3, &password.auth_bound_hmac_key_blob);
    out
}

fn decode_password_authenticator(bytes: &[u8]) -> Result<PasswordAuthenticator, UserStoreError> {
    let mut secure_user_id = 0;
    let mut gatekeeper_password_handle = Vec::new();
    let mut auth_bound_hmac_key_blob = Vec::new();
    read_fields(bytes, |field| {
        match field.number {
            1 => secure_user_id = field.varint()?,
            2 => gatekeeper_password_handle = field.bytes()?.to_vec(),
            3 => auth_bound_hmac_key_blob = field.bytes()?.to_vec(),
            _ => {}
        }
        Ok(())
    })?;
    Ok(PasswordAuthenticator {
        secure_user_id,
        gatekeeper_password_handle,
        auth_bound_hmac_key_blob,
    })
}

pub fn validate_user(user: &User) -> Result<(), UserStoreError> {
    validate_uid(user.record.uid)?;
    validate_name(&user.record.name)?;
    validate_display_name(&user.record.display_name)?;
    if user.filesystem.root_path != user_home_path(user.record.uid) || !user.filesystem.encrypted {
        return Err(UserStoreError::CorruptRecord);
    }
    if !user
        .authenticators
        .iter()
        .any(|a| a.kind == AuthenticatorKind::Password && a.password.is_some())
    {
        return Err(UserStoreError::MissingPasswordAuthenticator);
    }
    for authenticator in &user.authenticators {
        if authenticator.authenticator_id == 0 {
            return Err(UserStoreError::CorruptRecord);
        }
        if let Some(password) = &authenticator.password {
            validate_password_authenticator(password)?;
        }
    }
    Ok(())
}

fn validate_uid(uid: u64) -> Result<(), UserStoreError> {
    if uid == 0 {
        Err(UserStoreError::InvalidUid)
    } else {
        Ok(())
    }
}

fn validate_name(name: &str) -> Result<(), UserStoreError> {
    if name.is_empty()
        || name.len() > 64
        || name
            .bytes()
            .any(|b| !b.is_ascii_alphanumeric() && b != b'.' && b != b'-' && b != b'_')
    {
        Err(UserStoreError::InvalidName)
    } else {
        Ok(())
    }
}

fn validate_display_name(name: &str) -> Result<(), UserStoreError> {
    if name.is_empty() || name.len() > 128 || name.bytes().any(|b| b < 0x20 || b == 0x7f) {
        Err(UserStoreError::InvalidName)
    } else {
        Ok(())
    }
}

fn validate_password_authenticator(password: &PasswordAuthenticator) -> Result<(), UserStoreError> {
    if password.secure_user_id == 0
        || password.gatekeeper_password_handle.is_empty()
        || password.gatekeeper_password_handle.len() > 4096
        || password.auth_bound_hmac_key_blob.is_empty()
        || password.auth_bound_hmac_key_blob.len() > 4096
    {
        Err(UserStoreError::CorruptRecord)
    } else {
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct Field<'a> {
    number: u32,
    wire_type: u8,
    value: &'a [u8],
}

impl<'a> Field<'a> {
    fn string(self) -> Result<String, UserStoreError> {
        if self.wire_type != 2 {
            return Err(UserStoreError::CorruptRecord);
        }
        core::str::from_utf8(self.value)
            .map(str::to_string)
            .map_err(|_| UserStoreError::CorruptRecord)
    }

    fn bytes(self) -> Result<&'a [u8], UserStoreError> {
        if self.wire_type != 2 {
            return Err(UserStoreError::CorruptRecord);
        }
        Ok(self.value)
    }

    fn varint(self) -> Result<u64, UserStoreError> {
        if self.wire_type != 0 {
            return Err(UserStoreError::CorruptRecord);
        }
        decode_raw_varint(self.value).map(|(value, _)| value)
    }
}

fn read_fields<F>(bytes: &[u8], mut f: F) -> Result<(), UserStoreError>
where
    F: FnMut(Field<'_>) -> Result<(), UserStoreError>,
{
    let mut offset = 0;
    while offset < bytes.len() {
        let (key, key_len) = decode_raw_varint(&bytes[offset..])?;
        offset += key_len;
        let number = (key >> 3) as u32;
        let wire_type = (key & 0x7) as u8;
        match wire_type {
            0 => {
                let start = offset;
                let (_, len) = decode_raw_varint(&bytes[offset..])?;
                offset += len;
                f(Field {
                    number,
                    wire_type,
                    value: &bytes[start..offset],
                })?;
            }
            2 => {
                let (len, len_len) = decode_raw_varint(&bytes[offset..])?;
                offset += len_len;
                let len = usize::try_from(len).map_err(|_| UserStoreError::CorruptRecord)?;
                let end = offset
                    .checked_add(len)
                    .ok_or(UserStoreError::CorruptRecord)?;
                if end > bytes.len() {
                    return Err(UserStoreError::CorruptRecord);
                }
                f(Field {
                    number,
                    wire_type,
                    value: &bytes[offset..end],
                })?;
                offset = end;
            }
            _ => return Err(UserStoreError::CorruptRecord),
        }
    }
    Ok(())
}

fn put_message(out: &mut Vec<u8>, field: u32, value: &[u8]) {
    put_bytes(out, field, value);
}

fn put_string(out: &mut Vec<u8>, field: u32, value: &str) {
    put_bytes(out, field, value.as_bytes());
}

fn put_bytes(out: &mut Vec<u8>, field: u32, value: &[u8]) {
    put_raw_varint(out, u64::from(field << 3 | 2));
    put_raw_varint(out, value.len() as u64);
    out.extend_from_slice(value);
}

fn put_varint(out: &mut Vec<u8>, field: u32, value: u64) {
    put_raw_varint(out, u64::from(field << 3));
    put_raw_varint(out, value);
}

fn put_raw_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn decode_raw_varint(bytes: &[u8]) -> Result<(u64, usize), UserStoreError> {
    let mut value = 0u64;
    for (index, shift) in (0..64).step_by(7).enumerate() {
        let byte = *bytes.get(index).ok_or(UserStoreError::CorruptRecord)?;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok((value, index + 1));
        }
    }
    Err(UserStoreError::CorruptRecord)
}

#[cfg(feature = "redb_backend")]
pub mod persistent {
    use super::{
        CreateUserRequest, MemoryUserStore, UpdateUserRequest, User, UserStoreError, decode_user,
        encode_user, validate_user,
    };
    use alloc::vec::Vec;
    use bexos_redb::{BlockStore, open_or_create_with_store};
    use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};

    const USERS: TableDefinition<u64, &[u8]> = TableDefinition::new("users");

    pub struct UserStoreDb {
        db: Database,
    }

    impl UserStoreDb {
        pub fn open<S: BlockStore>(store: S) -> Result<Self, UserStoreError> {
            Ok(Self {
                db: open_or_create_with_store(store).map_err(|_| UserStoreError::Storage)?,
            })
        }

        pub fn list_users(&self) -> Result<Vec<User>, UserStoreError> {
            let tx = self.db.begin_read().map_err(|_| UserStoreError::Storage)?;
            let Ok(table) = tx.open_table(USERS) else {
                return Ok(Vec::new());
            };
            table
                .iter()
                .map_err(|_| UserStoreError::Storage)?
                .map(|entry| {
                    let (_, value) = entry.map_err(|_| UserStoreError::Storage)?;
                    decode_user(value.value())
                })
                .collect()
        }

        pub fn get_user(&self, uid: u64) -> Result<User, UserStoreError> {
            let tx = self.db.begin_read().map_err(|_| UserStoreError::Storage)?;
            let table = tx.open_table(USERS).map_err(|_| UserStoreError::NotFound)?;
            table
                .get(uid)
                .map_err(|_| UserStoreError::Storage)?
                .map(|value| decode_user(value.value()))
                .ok_or(UserStoreError::NotFound)?
        }

        pub fn create_user(&self, request: CreateUserRequest) -> Result<User, UserStoreError> {
            let mut memory = MemoryUserStore::new();
            for user in self.list_users()? {
                memory.import_decoded(user)?;
            }
            let user = memory.create_user(request)?;
            self.put_user(&user)?;
            Ok(user)
        }

        pub fn update_user(&self, request: UpdateUserRequest) -> Result<User, UserStoreError> {
            let mut user = self.get_user(request.uid)?;
            user.record.name = request.name;
            user.record.display_name = request.display_name;
            user.record.state = if request.disabled {
                super::UserState::Disabled
            } else {
                super::UserState::Ok
            };
            if let Some(handle) = request.gatekeeper_password_handle {
                let password = user
                    .authenticators
                    .iter_mut()
                    .find_map(|authenticator| authenticator.password.as_mut())
                    .ok_or(UserStoreError::MissingPasswordAuthenticator)?;
                password.gatekeeper_password_handle = handle;
            }
            user.record.updated_generation = request.generation;
            validate_user(&user)?;
            self.put_user(&user)?;
            Ok(user)
        }

        pub fn delete_user(&self, uid: u64) -> Result<User, UserStoreError> {
            let user = self.get_user(uid)?;
            let tx = self.db.begin_write().map_err(|_| UserStoreError::Storage)?;
            {
                let mut table = tx.open_table(USERS).map_err(|_| UserStoreError::Storage)?;
                table.remove(uid).map_err(|_| UserStoreError::Storage)?;
            }
            tx.commit().map_err(|_| UserStoreError::Storage)?;
            Ok(user)
        }

        fn put_user(&self, user: &User) -> Result<(), UserStoreError> {
            let bytes = encode_user(user);
            let tx = self.db.begin_write().map_err(|_| UserStoreError::Storage)?;
            {
                let mut table = tx.open_table(USERS).map_err(|_| UserStoreError::Storage)?;
                table
                    .insert(user.record.uid, bytes.as_slice())
                    .map_err(|_| UserStoreError::Storage)?;
            }
            tx.commit().map_err(|_| UserStoreError::Storage)
        }
    }
}
