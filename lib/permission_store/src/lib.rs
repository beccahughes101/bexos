#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PermissionRequirement {
    Required,
    Optional,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PermissionDeclaration {
    pub name: String,
    pub values: Vec<String>,
    pub requirement: PermissionRequirement,
    pub usage_description: String,
}

impl Default for PermissionRequirement {
    fn default() -> Self {
        Self::Required
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SystemGrantRecord {
    pub package_id: String,
    pub permission_name: String,
    pub allowed_values: Vec<String>,
    pub verified_at: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UserGrantState {
    Granted,
    Denied,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UserGrantRecord {
    pub uid: u64,
    pub package_id: String,
    pub permission_name: String,
    pub state: UserGrantState,
    pub granted_values: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PermissionStoreError {
    EmptyPackage,
    EmptyPermission,
    UndeclaredPermission,
    UndeclaredValue,
    CorruptRecord,
    InvalidUtf8,
    InvalidWireType(u8),
    InvalidVarint,
    LengthOverflow,
    Storage,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MemoryPermissionStore {
    system: Vec<SystemGrantRecord>,
    users: Vec<UserGrantRecord>,
}

impl MemoryPermissionStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_system_declarations(
        &mut self,
        package_id: &str,
        declarations: &[PermissionDeclaration],
        verified_at: u64,
    ) -> Result<(), PermissionStoreError> {
        validate_package(package_id)?;
        for declaration in declarations {
            validate_permission(&declaration.name)?;
            self.upsert_system(SystemGrantRecord {
                package_id: package_id.to_string(),
                permission_name: declaration.name.clone(),
                allowed_values: declaration.values.clone(),
                verified_at,
            });
        }
        Ok(())
    }

    pub fn auto_grant_required(
        &mut self,
        uid: u64,
        package_id: &str,
        declarations: &[PermissionDeclaration],
    ) -> Result<(), PermissionStoreError> {
        for declaration in declarations {
            if declaration.requirement == PermissionRequirement::Required {
                self.grant_declared(uid, package_id, declaration, &declaration.values)?;
            }
        }
        Ok(())
    }

    pub fn grant_declared(
        &mut self,
        uid: u64,
        package_id: &str,
        declaration: &PermissionDeclaration,
        requested_values: &[String],
    ) -> Result<UserGrantRecord, PermissionStoreError> {
        validate_package(package_id)?;
        validate_permission(&declaration.name)?;
        if !values_are_subset(requested_values, &declaration.values) {
            return Err(PermissionStoreError::UndeclaredValue);
        }
        let granted_values = if requested_values.is_empty() {
            declaration.values.clone()
        } else {
            requested_values.to_vec()
        };
        let record = UserGrantRecord {
            uid,
            package_id: package_id.to_string(),
            permission_name: declaration.name.clone(),
            state: UserGrantState::Granted,
            granted_values,
        };
        self.upsert_user(record.clone());
        Ok(record)
    }

    pub fn remove_package(&mut self, package_id: &str) {
        self.system.retain(|record| record.package_id != package_id);
        self.users.retain(|record| record.package_id != package_id);
    }

    pub fn remove_user(&mut self, uid: u64) {
        self.users.retain(|record| record.uid != uid);
    }

    pub fn replace_user_records(
        &mut self,
        uid: u64,
        records: Vec<UserGrantRecord>,
    ) -> Result<(), PermissionStoreError> {
        for record in &records {
            if record.uid != uid {
                return Err(PermissionStoreError::CorruptRecord);
            }
            validate_package(&record.package_id)?;
            validate_permission(&record.permission_name)?;
        }
        self.remove_user(uid);
        for record in records {
            self.upsert_user(record);
        }
        Ok(())
    }

    pub fn retain_user_records<F>(&mut self, mut keep: F)
    where
        F: FnMut(&UserGrantRecord) -> bool,
    {
        self.users.retain(|record| keep(record));
    }

    pub fn user_grants(&self, uid: u64, package_id: &str) -> Vec<UserGrantRecord> {
        self.users
            .iter()
            .filter(|record| {
                record.uid == uid
                    && record.package_id == package_id
                    && record.state == UserGrantState::Granted
            })
            .cloned()
            .collect()
    }

    pub fn user_records(&self, uid: u64) -> Vec<UserGrantRecord> {
        self.users
            .iter()
            .filter(|record| record.uid == uid)
            .cloned()
            .collect()
    }

    pub fn all_user_records(&self) -> &[UserGrantRecord] {
        &self.users
    }

    pub fn granted_values(&self, uid: u64, package_id: &str, permission_name: &str) -> Vec<String> {
        self.users
            .iter()
            .find(|record| {
                record.uid == uid
                    && record.package_id == package_id
                    && record.permission_name == permission_name
                    && record.state == UserGrantState::Granted
            })
            .map(|record| record.granted_values.clone())
            .unwrap_or_default()
    }

    pub fn system_grants(&self) -> &[SystemGrantRecord] {
        &self.system
    }

    pub fn replace_system_grants(
        &mut self,
        records: Vec<SystemGrantRecord>,
    ) -> Result<(), PermissionStoreError> {
        self.system.clear();
        for record in records {
            validate_package(&record.package_id)?;
            validate_permission(&record.permission_name)?;
            self.upsert_system(record);
        }
        Ok(())
    }

    #[cfg(feature = "redb_backend")]
    pub fn merge_from_memory(&mut self, current: &MemoryPermissionStore) {
        for record in current.system_grants() {
            if !self.system.iter().any(|existing| {
                existing.package_id == record.package_id
                    && existing.permission_name == record.permission_name
            }) {
                self.system.push(record.clone());
            }
        }
        for record in &current.users {
            if !self.users.iter().any(|existing| {
                existing.uid == record.uid
                    && existing.package_id == record.package_id
                    && existing.permission_name == record.permission_name
            }) {
                self.users.push(record.clone());
            }
        }
    }

    fn upsert_system(&mut self, record: SystemGrantRecord) {
        if let Some(existing) = self.system.iter_mut().find(|existing| {
            existing.package_id == record.package_id
                && existing.permission_name == record.permission_name
        }) {
            *existing = record;
        } else {
            self.system.push(record);
            self.system.sort_by(|left, right| {
                (&left.package_id, &left.permission_name)
                    .cmp(&(&right.package_id, &right.permission_name))
            });
        }
    }

    fn upsert_user(&mut self, record: UserGrantRecord) {
        if let Some(existing) = self.users.iter_mut().find(|existing| {
            existing.uid == record.uid
                && existing.package_id == record.package_id
                && existing.permission_name == record.permission_name
        }) {
            *existing = record;
        } else {
            self.users.push(record);
            self.users.sort_by(|left, right| {
                (left.uid, &left.package_id, &left.permission_name).cmp(&(
                    right.uid,
                    &right.package_id,
                    &right.permission_name,
                ))
            });
        }
    }
}

pub fn values_are_subset(requested: &[String], declared: &[String]) -> bool {
    requested
        .iter()
        .all(|value| declared.iter().any(|declared| declared == value))
}

#[cfg(feature = "redb_backend")]
pub mod persistent {
    use super::{
        MemoryPermissionStore, PermissionDeclaration, PermissionStoreError, SystemGrantRecord,
        UserGrantRecord, UserGrantState, decode_system_record, decode_user_record,
        encode_system_record, encode_user_record,
    };
    use alloc::format;
    use alloc::string::{String, ToString};
    use alloc::vec::Vec;
    use bexos_redb::{BlockStore, open_or_create_with_store};
    use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};

    const SYSTEM: TableDefinition<&str, &[u8]> = TableDefinition::new("system_permissions");
    const USERS: TableDefinition<&str, &[u8]> = TableDefinition::new("user_permissions");

    pub struct PermissionStoreDb {
        db: Database,
    }

    impl PermissionStoreDb {
        pub fn open<S: BlockStore>(store: S) -> Result<Self, PermissionStoreError> {
            Self::open_detailed(store).map_err(|_| PermissionStoreError::Storage)
        }

        pub fn open_detailed<S: BlockStore>(store: S) -> Result<Self, redb::DatabaseError> {
            Ok(Self {
                db: open_or_create_with_store(store)?,
            })
        }

        pub fn register_system_declarations(
            &self,
            package_id: &str,
            declarations: &[PermissionDeclaration],
            verified_at: u64,
        ) -> Result<(), PermissionStoreError> {
            let mut memory = MemoryPermissionStore::new();
            memory.register_system_declarations(package_id, declarations, verified_at)?;
            let tx = self
                .db
                .begin_write()
                .map_err(|_| PermissionStoreError::Storage)?;
            {
                let mut table = tx
                    .open_table(SYSTEM)
                    .map_err(|_| PermissionStoreError::Storage)?;
                for record in memory.system_grants() {
                    table
                        .insert(
                            system_key(package_id, &record.permission_name).as_str(),
                            encode_system_record(record).as_slice(),
                        )
                        .map_err(|_| PermissionStoreError::Storage)?;
                }
            }
            tx.commit().map_err(|_| PermissionStoreError::Storage)
        }

        pub fn grant_declared(
            &self,
            uid: u64,
            package_id: &str,
            declaration: &PermissionDeclaration,
            requested_values: &[String],
        ) -> Result<UserGrantRecord, PermissionStoreError> {
            let mut memory = MemoryPermissionStore::new();
            let record = memory.grant_declared(uid, package_id, declaration, requested_values)?;
            let tx = self
                .db
                .begin_write()
                .map_err(|_| PermissionStoreError::Storage)?;
            {
                let mut table = tx
                    .open_table(USERS)
                    .map_err(|_| PermissionStoreError::Storage)?;
                table
                    .insert(
                        user_key(uid, package_id, &record.permission_name).as_str(),
                        encode_user_record(&record).as_slice(),
                    )
                    .map_err(|_| PermissionStoreError::Storage)?;
            }
            tx.commit().map_err(|_| PermissionStoreError::Storage)?;
            Ok(record)
        }

        pub fn user_grants(
            &self,
            uid: u64,
            package_id: &str,
        ) -> Result<Vec<UserGrantRecord>, PermissionStoreError> {
            let tx = self
                .db
                .begin_read()
                .map_err(|_| PermissionStoreError::Storage)?;
            let Ok(table) = tx.open_table(USERS) else {
                return Ok(Vec::new());
            };
            let prefix = format!("{uid}|{package_id}|");
            table
                .iter()
                .map_err(|_| PermissionStoreError::Storage)?
                .filter_map(|entry| {
                    let (key, value) = entry.ok()?;
                    if key.value().starts_with(&prefix) {
                        Some(decode_user_record(value.value()))
                    } else {
                        None
                    }
                })
                .collect()
        }

        pub fn remove_package(&self, package_id: &str) -> Result<(), PermissionStoreError> {
            let tx = self
                .db
                .begin_write()
                .map_err(|_| PermissionStoreError::Storage)?;
            {
                let mut system = tx
                    .open_table(SYSTEM)
                    .map_err(|_| PermissionStoreError::Storage)?;
                let keys = system
                    .iter()
                    .map_err(|_| PermissionStoreError::Storage)?
                    .filter_map(|entry| {
                        let (key, _) = entry.ok()?;
                        key.value()
                            .starts_with(package_id)
                            .then(|| key.value().to_string())
                    })
                    .collect::<Vec<_>>();
                for key in keys {
                    system
                        .remove(key.as_str())
                        .map_err(|_| PermissionStoreError::Storage)?;
                }
            }
            {
                let mut users = tx
                    .open_table(USERS)
                    .map_err(|_| PermissionStoreError::Storage)?;
                let keys = users
                    .iter()
                    .map_err(|_| PermissionStoreError::Storage)?
                    .filter_map(|entry| {
                        let (key, _) = entry.ok()?;
                        key.value()
                            .contains(&format!("|{package_id}|"))
                            .then(|| key.value().to_string())
                    })
                    .collect::<Vec<_>>();
                for key in keys {
                    users
                        .remove(key.as_str())
                        .map_err(|_| PermissionStoreError::Storage)?;
                }
            }
            tx.commit().map_err(|_| PermissionStoreError::Storage)
        }

        pub fn system_grants(&self) -> Result<Vec<SystemGrantRecord>, PermissionStoreError> {
            let tx = self
                .db
                .begin_read()
                .map_err(|_| PermissionStoreError::Storage)?;
            let Ok(table) = tx.open_table(SYSTEM) else {
                return Ok(Vec::new());
            };
            table
                .iter()
                .map_err(|_| PermissionStoreError::Storage)?
                .map(|entry| {
                    let (_, value) = entry.map_err(|_| PermissionStoreError::Storage)?;
                    decode_system_record(value.value())
                })
                .collect()
        }

        pub fn snapshot_memory(&self) -> Result<MemoryPermissionStore, PermissionStoreError> {
            let mut memory = MemoryPermissionStore::new();
            memory.system = self.system_grants()?;
            let tx = self
                .db
                .begin_read()
                .map_err(|_| PermissionStoreError::Storage)?;
            if let Ok(table) = tx.open_table(USERS) {
                memory.users = table
                    .iter()
                    .map_err(|_| PermissionStoreError::Storage)?
                    .map(|entry| {
                        let (_, value) = entry.map_err(|_| PermissionStoreError::Storage)?;
                        decode_user_record(value.value())
                    })
                    .collect::<Result<Vec<_>, _>>()?;
            }
            Ok(memory)
        }

        pub fn snapshot_system_memory(
            &self,
        ) -> Result<MemoryPermissionStore, PermissionStoreError> {
            let mut memory = MemoryPermissionStore::new();
            memory.system = self.system_grants()?;
            memory.users = self.user_records_for_uid(0)?;
            Ok(memory)
        }

        pub fn snapshot_user_memory(
            &self,
            uid: u64,
        ) -> Result<MemoryPermissionStore, PermissionStoreError> {
            let mut memory = MemoryPermissionStore::new();
            memory.users = self.user_records_for_uid(uid)?;
            Ok(memory)
        }

        pub fn replace_from_memory(
            &self,
            memory: &MemoryPermissionStore,
        ) -> Result<(), PermissionStoreError> {
            let tx = self
                .db
                .begin_write()
                .map_err(|_| PermissionStoreError::Storage)?;
            {
                let mut system = tx
                    .open_table(SYSTEM)
                    .map_err(|_| PermissionStoreError::Storage)?;
                system
                    .retain(|_, _| false)
                    .map_err(|_| PermissionStoreError::Storage)?;
                for record in memory.system_grants() {
                    system
                        .insert(
                            system_key(&record.package_id, &record.permission_name).as_str(),
                            encode_system_record(record).as_slice(),
                        )
                        .map_err(|_| PermissionStoreError::Storage)?;
                }
            }
            {
                let mut users = tx
                    .open_table(USERS)
                    .map_err(|_| PermissionStoreError::Storage)?;
                users
                    .retain(|_, _| false)
                    .map_err(|_| PermissionStoreError::Storage)?;
                for record in &memory.users {
                    users
                        .insert(
                            user_key(record.uid, &record.package_id, &record.permission_name)
                                .as_str(),
                            encode_user_record(record).as_slice(),
                        )
                        .map_err(|_| PermissionStoreError::Storage)?;
                }
            }
            tx.commit().map_err(|_| PermissionStoreError::Storage)
        }

        pub fn replace_system_from_memory(
            &self,
            memory: &MemoryPermissionStore,
        ) -> Result<(), PermissionStoreError> {
            let tx = self
                .db
                .begin_write()
                .map_err(|_| PermissionStoreError::Storage)?;
            {
                let mut system = tx
                    .open_table(SYSTEM)
                    .map_err(|_| PermissionStoreError::Storage)?;
                system
                    .retain(|_, _| false)
                    .map_err(|_| PermissionStoreError::Storage)?;
                for record in memory.system_grants() {
                    system
                        .insert(
                            system_key(&record.package_id, &record.permission_name).as_str(),
                            encode_system_record(record).as_slice(),
                        )
                        .map_err(|_| PermissionStoreError::Storage)?;
                }
            }
            {
                let mut users = tx
                    .open_table(USERS)
                    .map_err(|_| PermissionStoreError::Storage)?;
                users
                    .retain(|_, _| false)
                    .map_err(|_| PermissionStoreError::Storage)?;
                for record in memory
                    .all_user_records()
                    .iter()
                    .filter(|record| record.uid == 0)
                {
                    users
                        .insert(
                            user_key(record.uid, &record.package_id, &record.permission_name)
                                .as_str(),
                            encode_user_record(record).as_slice(),
                        )
                        .map_err(|_| PermissionStoreError::Storage)?;
                }
            }
            tx.commit().map_err(|_| PermissionStoreError::Storage)
        }

        pub fn replace_user_from_memory(
            &self,
            uid: u64,
            memory: &MemoryPermissionStore,
        ) -> Result<(), PermissionStoreError> {
            let tx = self
                .db
                .begin_write()
                .map_err(|_| PermissionStoreError::Storage)?;
            {
                let mut users = tx
                    .open_table(USERS)
                    .map_err(|_| PermissionStoreError::Storage)?;
                let prefix = format!("{uid}|");
                users
                    .retain(|key, _| !key.starts_with(&prefix))
                    .map_err(|_| PermissionStoreError::Storage)?;
                for record in memory
                    .all_user_records()
                    .iter()
                    .filter(|record| record.uid == uid)
                {
                    users
                        .insert(
                            user_key(record.uid, &record.package_id, &record.permission_name)
                                .as_str(),
                            encode_user_record(record).as_slice(),
                        )
                        .map_err(|_| PermissionStoreError::Storage)?;
                }
            }
            tx.commit().map_err(|_| PermissionStoreError::Storage)
        }

        pub fn scrub_non_system_user_rows(&self) -> Result<(), PermissionStoreError> {
            let tx = self
                .db
                .begin_write()
                .map_err(|_| PermissionStoreError::Storage)?;
            {
                let mut users = tx
                    .open_table(USERS)
                    .map_err(|_| PermissionStoreError::Storage)?;
                users
                    .retain(|key, _| key.starts_with("0|"))
                    .map_err(|_| PermissionStoreError::Storage)?;
            }
            tx.commit().map_err(|_| PermissionStoreError::Storage)
        }

        pub fn remove_user(&self, uid: u64) -> Result<(), PermissionStoreError> {
            let tx = self
                .db
                .begin_write()
                .map_err(|_| PermissionStoreError::Storage)?;
            {
                let mut users = tx
                    .open_table(USERS)
                    .map_err(|_| PermissionStoreError::Storage)?;
                let prefix = format!("{uid}|");
                users
                    .retain(|key, _| !key.starts_with(&prefix))
                    .map_err(|_| PermissionStoreError::Storage)?;
            }
            tx.commit().map_err(|_| PermissionStoreError::Storage)
        }

        fn user_records_for_uid(
            &self,
            uid: u64,
        ) -> Result<Vec<UserGrantRecord>, PermissionStoreError> {
            let tx = self
                .db
                .begin_read()
                .map_err(|_| PermissionStoreError::Storage)?;
            let Ok(table) = tx.open_table(USERS) else {
                return Ok(Vec::new());
            };
            let prefix = format!("{uid}|");
            table
                .iter()
                .map_err(|_| PermissionStoreError::Storage)?
                .filter_map(|entry| {
                    let (key, value) = entry.ok()?;
                    if key.value().starts_with(&prefix) {
                        Some(decode_user_record(value.value()))
                    } else {
                        None
                    }
                })
                .collect()
        }
    }

    fn system_key(package_id: &str, permission_name: &str) -> String {
        format!("{package_id}|{permission_name}")
    }

    fn user_key(uid: u64, package_id: &str, permission_name: &str) -> String {
        format!("{uid}|{package_id}|{permission_name}")
    }

    #[allow(dead_code)]
    fn _assert_state_used(state: UserGrantState) -> UserGrantState {
        state
    }
}

#[cfg(feature = "redb_backend")]
fn encode_system_record(record: &SystemGrantRecord) -> Vec<u8> {
    let mut out = Vec::new();
    put_string(&mut out, 1, &record.package_id);
    put_string(&mut out, 2, &record.permission_name);
    for value in &record.allowed_values {
        put_string(&mut out, 3, value);
    }
    put_varint(&mut out, 4, record.verified_at);
    out
}

#[cfg(feature = "redb_backend")]
fn decode_system_record(bytes: &[u8]) -> Result<SystemGrantRecord, PermissionStoreError> {
    let mut record = SystemGrantRecord {
        package_id: String::new(),
        permission_name: String::new(),
        allowed_values: Vec::new(),
        verified_at: 0,
    };
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => record.package_id = field.string()?,
            2 => record.permission_name = field.string()?,
            3 => record.allowed_values.push(field.string()?),
            4 => record.verified_at = field.varint()?,
            _ => {}
        }
    }
    validate_package(&record.package_id)?;
    validate_permission(&record.permission_name)?;
    Ok(record)
}

#[cfg(feature = "redb_backend")]
fn encode_user_record(record: &UserGrantRecord) -> Vec<u8> {
    let mut out = Vec::new();
    put_varint(&mut out, 1, record.uid as u64);
    put_string(&mut out, 2, &record.package_id);
    put_string(&mut out, 3, &record.permission_name);
    put_varint(
        &mut out,
        4,
        match record.state {
            UserGrantState::Granted => 1,
            UserGrantState::Denied => 2,
        },
    );
    for value in &record.granted_values {
        put_string(&mut out, 5, value);
    }
    out
}

#[cfg(feature = "redb_backend")]
fn decode_user_record(bytes: &[u8]) -> Result<UserGrantRecord, PermissionStoreError> {
    let mut record = UserGrantRecord {
        uid: 0,
        package_id: String::new(),
        permission_name: String::new(),
        state: UserGrantState::Denied,
        granted_values: Vec::new(),
    };
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => record.uid = field.varint()?,
            2 => record.package_id = field.string()?,
            3 => record.permission_name = field.string()?,
            4 => {
                record.state = match field.varint()? {
                    1 => UserGrantState::Granted,
                    2 => UserGrantState::Denied,
                    _ => return Err(PermissionStoreError::CorruptRecord),
                }
            }
            5 => record.granted_values.push(field.string()?),
            _ => {}
        }
    }
    validate_package(&record.package_id)?;
    validate_permission(&record.permission_name)?;
    Ok(record)
}

fn validate_package(package_id: &str) -> Result<(), PermissionStoreError> {
    if package_id.is_empty() {
        Err(PermissionStoreError::EmptyPackage)
    } else {
        Ok(())
    }
}

fn validate_permission(permission: &str) -> Result<(), PermissionStoreError> {
    if permission.is_empty() {
        Err(PermissionStoreError::EmptyPermission)
    } else {
        Ok(())
    }
}

#[cfg(feature = "redb_backend")]
struct Field<'a> {
    number: u32,
    wire_type: u8,
    bytes: &'a [u8],
}

#[cfg(feature = "redb_backend")]
impl<'a> Field<'a> {
    fn string(&self) -> Result<String, PermissionStoreError> {
        if self.wire_type != 2 {
            return Err(PermissionStoreError::InvalidWireType(self.wire_type));
        }
        Ok(core::str::from_utf8(self.bytes)
            .map_err(|_| PermissionStoreError::InvalidUtf8)?
            .to_string())
    }

    fn varint(&self) -> Result<u64, PermissionStoreError> {
        if self.wire_type != 0 {
            return Err(PermissionStoreError::InvalidWireType(self.wire_type));
        }
        let mut value = 0u64;
        for (shift, byte) in self.bytes.iter().enumerate() {
            value |= u64::from(byte & 0x7f) << (shift * 7);
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(PermissionStoreError::InvalidVarint)
    }
}

#[cfg(feature = "redb_backend")]
struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

#[cfg(feature = "redb_backend")]
impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn next_field(&mut self) -> Result<Option<Field<'a>>, PermissionStoreError> {
        if self.pos == self.bytes.len() {
            return Ok(None);
        }
        let key = self.read_varint()?;
        let wire_type = (key & 0x7) as u8;
        let number = (key >> 3) as u32;
        let (value_start, value_end) = match wire_type {
            0 => {
                let value_start = self.pos;
                self.read_varint()?;
                (value_start, self.pos)
            }
            2 => {
                let len = self.read_varint()? as usize;
                let value_start = self.pos;
                self.pos = self
                    .pos
                    .checked_add(len)
                    .ok_or(PermissionStoreError::LengthOverflow)?;
                if self.pos > self.bytes.len() {
                    return Err(PermissionStoreError::LengthOverflow);
                }
                (value_start, self.pos)
            }
            other => return Err(PermissionStoreError::InvalidWireType(other)),
        };
        Ok(Some(Field {
            number,
            wire_type,
            bytes: &self.bytes[value_start..value_end],
        }))
    }

    fn read_varint(&mut self) -> Result<u64, PermissionStoreError> {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = *self
                .bytes
                .get(self.pos)
                .ok_or(PermissionStoreError::InvalidVarint)?;
            self.pos += 1;
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(PermissionStoreError::InvalidVarint)
    }
}

#[cfg(feature = "redb_backend")]
fn put_string(out: &mut Vec<u8>, number: u32, value: &str) {
    put_key(out, number, 2);
    put_raw_varint(out, value.len() as u64);
    out.extend_from_slice(value.as_bytes());
}

#[cfg(feature = "redb_backend")]
fn put_varint(out: &mut Vec<u8>, number: u32, value: u64) {
    put_key(out, number, 0);
    put_raw_varint(out, value);
}

#[cfg(feature = "redb_backend")]
fn put_key(out: &mut Vec<u8>, number: u32, wire_type: u8) {
    put_raw_varint(out, (u64::from(number) << 3) | u64::from(wire_type));
}

#[cfg(feature = "redb_backend")]
fn put_raw_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}
