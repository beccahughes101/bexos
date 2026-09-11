#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpenerScope {
    System,
    User(u64),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HandlerId {
    pub package: String,
    pub process: String,
}

impl HandlerId {
    pub fn new(package: impl Into<String>, process: impl Into<String>) -> Self {
        Self {
            package: package.into(),
            process: process.into(),
        }
    }

    #[cfg(feature = "redb_backend")]
    fn key(&self) -> String {
        let mut key = self.package.clone();
        key.push(':');
        key.push_str(&self.process);
        key
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HandlerRegistration {
    pub package: String,
    pub process: String,
    pub schemes: Vec<String>,
    pub domains: Vec<String>,
    pub mime_types: Vec<String>,
    pub interfaces: Vec<String>,
    pub domains_verified: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedHandler {
    pub package: String,
    pub process: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpenKind<'a> {
    Url(&'a str),
    Mime(&'a str),
    Interface(&'a str),
    App(&'a str),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResolveOutcome {
    Selected(ResolvedHandler),
    PromptPendingUser(Vec<ResolvedHandler>),
    NoHandler,
    AccessDenied,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OpenerStoreError {
    EmptyPackage,
    EmptyProcess,
    EmptyKey,
    InvalidKey,
    CorruptRecord,
    Storage,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MemoryOpenerRegistry {
    system: Vec<HandlerRegistration>,
    users: Vec<UserHandlerRegistration>,
    defaults: Vec<UserDefault>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct UserHandlerRegistration {
    uid: u64,
    registration: HandlerRegistration,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct UserDefault {
    uid: u64,
    key: String,
    handler: HandlerId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UserHandlerSnapshot {
    pub uid: u64,
    pub registration: HandlerRegistration,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UserDefaultSnapshot {
    pub uid: u64,
    pub key: String,
    pub handler: HandlerId,
}

impl MemoryOpenerRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &mut self,
        scope: OpenerScope,
        registration: HandlerRegistration,
    ) -> Result<(), OpenerStoreError> {
        validate_registration(&registration)?;
        match scope {
            OpenerScope::System => upsert_registration(&mut self.system, registration),
            OpenerScope::User(uid) => {
                if let Some(existing) = self.users.iter_mut().find(|entry| {
                    entry.uid == uid
                        && entry.registration.package == registration.package
                        && entry.registration.process == registration.process
                }) {
                    existing.registration = registration;
                } else {
                    self.users
                        .push(UserHandlerRegistration { uid, registration });
                    self.users.sort_by(|left, right| {
                        (
                            left.uid,
                            left.registration.package.as_str(),
                            left.registration.process.as_str(),
                        )
                            .cmp(&(
                                right.uid,
                                right.registration.package.as_str(),
                                right.registration.process.as_str(),
                            ))
                    });
                }
            }
        }
        Ok(())
    }

    pub fn remove_package(&mut self, package: &str) {
        self.system.retain(|entry| entry.package != package);
        self.users
            .retain(|entry| entry.registration.package != package);
        self.defaults
            .retain(|entry| entry.handler.package != package);
    }

    pub fn set_user_default(
        &mut self,
        uid: u64,
        key: impl Into<String>,
        handler: HandlerId,
    ) -> Result<(), OpenerStoreError> {
        let key = key.into();
        validate_default_key(&key)?;
        if handler.package.is_empty() {
            return Err(OpenerStoreError::EmptyPackage);
        }
        if handler.process.is_empty() {
            return Err(OpenerStoreError::EmptyProcess);
        }
        if let Some(existing) = self
            .defaults
            .iter_mut()
            .find(|entry| entry.uid == uid && entry.key == key)
        {
            existing.handler = handler;
        } else {
            self.defaults.push(UserDefault { uid, key, handler });
            self.defaults
                .sort_by(|left, right| (left.uid, &left.key).cmp(&(right.uid, &right.key)));
        }
        Ok(())
    }

    pub fn resolve(&self, scope: OpenerScope, kind: OpenKind<'_>) -> ResolveOutcome {
        self.resolve_with_default(scope, kind, None)
    }

    /// A platform default applies only while its handler is installed; explicit
    /// per-user choices always win. Configuration is owned by the platform.
    pub fn resolve_with_default(
        &self,
        scope: OpenerScope,
        kind: OpenKind<'_>,
        fallback: Option<&HandlerId>,
    ) -> ResolveOutcome {
        let user_scope = match scope {
            OpenerScope::System => None,
            OpenerScope::User(uid) => Some(uid),
        };
        if let Some(uid) = user_scope {
            if let Some(default_key) = default_key(kind) {
                if let Some(default) = self
                    .defaults
                    .iter()
                    .find(|entry| entry.uid == uid && entry.key == default_key)
                {
                    if self.handler_registered(Some(uid), &default.handler, kind)
                        || self.handler_registered(None, &default.handler, kind)
                    {
                        return ResolveOutcome::Selected(ResolvedHandler {
                            package: default.handler.package.clone(),
                            process: default.handler.process.clone(),
                        });
                    }
                }
            }
            if let Some(handler) = fallback.filter(|h| self.handler_registered(None, h, kind)) {
                return ResolveOutcome::Selected(ResolvedHandler {
                    package: handler.package.clone(),
                    process: handler.process.clone(),
                });
            }
            let matches = self.user_matches(uid, kind);
            if !matches.is_empty() {
                return selected_or_prompt(matches);
            }
        }
        if let Some(handler) = fallback.filter(|h| self.handler_registered(None, h, kind)) {
            return ResolveOutcome::Selected(ResolvedHandler {
                package: handler.package.clone(),
                process: handler.process.clone(),
            });
        }
        selected_or_prompt(self.system_matches(kind))
    }

    #[cfg(feature = "redb_backend")]
    pub fn merge_from_memory(&mut self, current: &MemoryOpenerRegistry) {
        for registration in &current.system {
            let _ = self.register(OpenerScope::System, registration.clone());
        }
        for entry in &current.users {
            let _ = self.register(OpenerScope::User(entry.uid), entry.registration.clone());
        }
        for entry in &current.defaults {
            let _ = self.set_user_default(entry.uid, &entry.key, entry.handler.clone());
        }
    }

    pub fn system_handlers(&self) -> &[HandlerRegistration] {
        &self.system
    }

    pub fn user_handler_count(&self) -> usize {
        self.users.len()
    }

    pub fn user_handlers(&self) -> Vec<UserHandlerSnapshot> {
        self.users
            .iter()
            .map(|entry| UserHandlerSnapshot {
                uid: entry.uid,
                registration: entry.registration.clone(),
            })
            .collect()
    }

    pub fn user_defaults(&self) -> Vec<UserDefaultSnapshot> {
        self.defaults
            .iter()
            .map(|entry| UserDefaultSnapshot {
                uid: entry.uid,
                key: entry.key.clone(),
                handler: entry.handler.clone(),
            })
            .collect()
    }

    fn handler_registered(
        &self,
        uid: Option<u64>,
        handler: &HandlerId,
        kind: OpenKind<'_>,
    ) -> bool {
        match uid {
            Some(uid) => self.users.iter().any(|entry| {
                entry.uid == uid
                    && entry.registration.package == handler.package
                    && entry.registration.process == handler.process
                    && registration_matches(&entry.registration, kind)
            }),
            None => self.system.iter().any(|entry| {
                entry.package == handler.package
                    && entry.process == handler.process
                    && registration_matches(entry, kind)
            }),
        }
    }

    fn user_matches(&self, uid: u64, kind: OpenKind<'_>) -> Vec<ResolvedHandler> {
        let mut matches = self
            .users
            .iter()
            .filter(|entry| entry.uid == uid)
            .map(|entry| &entry.registration)
            .filter(|entry| registration_matches(entry, kind))
            .map(resolved)
            .collect::<Vec<_>>();
        sort_dedupe_handlers(&mut matches);
        matches
    }

    fn system_matches(&self, kind: OpenKind<'_>) -> Vec<ResolvedHandler> {
        let mut matches = self
            .system
            .iter()
            .filter(|entry| registration_matches(entry, kind))
            .map(resolved)
            .collect::<Vec<_>>();
        sort_dedupe_handlers(&mut matches);
        matches
    }
}

fn upsert_registration(entries: &mut Vec<HandlerRegistration>, registration: HandlerRegistration) {
    if let Some(existing) = entries.iter_mut().find(|entry| {
        entry.package == registration.package && entry.process == registration.process
    }) {
        *existing = registration;
    } else {
        entries.push(registration);
        entries.sort_by(|left, right| {
            (left.package.as_str(), left.process.as_str())
                .cmp(&(right.package.as_str(), right.process.as_str()))
        });
    }
}

fn selected_or_prompt(matches: Vec<ResolvedHandler>) -> ResolveOutcome {
    match matches.len() {
        0 => ResolveOutcome::NoHandler,
        1 => ResolveOutcome::Selected(matches[0].clone()),
        _ => ResolveOutcome::PromptPendingUser(matches),
    }
}

fn resolved(registration: &HandlerRegistration) -> ResolvedHandler {
    ResolvedHandler {
        package: registration.package.clone(),
        process: registration.process.clone(),
    }
}

fn sort_dedupe_handlers(matches: &mut Vec<ResolvedHandler>) {
    matches.sort_by(|left, right| {
        (left.package.as_str(), left.process.as_str())
            .cmp(&(right.package.as_str(), right.process.as_str()))
    });
    matches.dedup_by(|left, right| left.package == right.package && left.process == right.process);
}

fn registration_matches(registration: &HandlerRegistration, kind: OpenKind<'_>) -> bool {
    match kind {
        OpenKind::Url(url) => {
            url_scheme(url)
                .map(|scheme| registration.schemes.iter().any(|value| value == scheme))
                .unwrap_or(false)
                || verified_domain_match(registration, url)
        }
        OpenKind::Mime(mime) => registration
            .mime_types
            .iter()
            .any(|candidate| mime_matches(candidate, mime)),
        OpenKind::Interface(name) => registration.interfaces.iter().any(|value| value == name),
        OpenKind::App(package) => registration.package == package,
    }
}

fn verified_domain_match(registration: &HandlerRegistration, url: &str) -> bool {
    if !registration.domains_verified {
        return false;
    }
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let host = rest.split('/').next().unwrap_or(rest);
    registration.domains.iter().any(|domain| domain == host)
}

fn mime_matches(candidate: &str, requested: &str) -> bool {
    if candidate == requested {
        return true;
    }
    let Some((candidate_type, candidate_subtype)) = candidate.split_once('/') else {
        return false;
    };
    let Some((requested_type, _)) = requested.split_once('/') else {
        return false;
    };
    candidate_subtype == "*" && candidate_type == requested_type
}

fn url_scheme(url: &str) -> Option<&str> {
    let (scheme, _) = url.split_once(':')?;
    if scheme.is_empty()
        || scheme
            .bytes()
            .any(|b| !b.is_ascii_alphanumeric() && b != b'+' && b != b'-' && b != b'.')
    {
        return None;
    }
    Some(scheme)
}

fn default_key(kind: OpenKind<'_>) -> Option<String> {
    match kind {
        OpenKind::Url(url) => url_scheme(url).map(|scheme| {
            let mut key = String::from("scheme:");
            key.push_str(scheme);
            key
        }),
        OpenKind::Mime(mime) => {
            let mut key = String::from("mime:");
            key.push_str(mime);
            Some(key)
        }
        OpenKind::Interface(name) => {
            let mut key = String::from("interface:");
            key.push_str(name);
            Some(key)
        }
        OpenKind::App(_) => None,
    }
}

fn validate_registration(registration: &HandlerRegistration) -> Result<(), OpenerStoreError> {
    if registration.package.is_empty() {
        return Err(OpenerStoreError::EmptyPackage);
    }
    if registration.process.is_empty() {
        return Err(OpenerStoreError::EmptyProcess);
    }
    Ok(())
}

fn validate_default_key(key: &str) -> Result<(), OpenerStoreError> {
    if key.is_empty() {
        return Err(OpenerStoreError::EmptyKey);
    }
    if !(key.starts_with("mime:") || key.starts_with("scheme:") || key.starts_with("interface:")) {
        return Err(OpenerStoreError::InvalidKey);
    }
    Ok(())
}

#[cfg(feature = "redb_backend")]
fn encode_registration(registration: &HandlerRegistration) -> Vec<u8> {
    let mut out = Vec::new();
    put_string(&mut out, 1, &registration.package);
    put_string(&mut out, 2, &registration.process);
    for value in &registration.schemes {
        put_string(&mut out, 3, value);
    }
    for value in &registration.domains {
        put_string(&mut out, 4, value);
    }
    for value in &registration.mime_types {
        put_string(&mut out, 5, value);
    }
    for value in &registration.interfaces {
        put_string(&mut out, 6, value);
    }
    put_varint(&mut out, 7, registration.domains_verified as u64);
    out
}

#[cfg(feature = "redb_backend")]
fn decode_registration(bytes: &[u8]) -> Result<HandlerRegistration, OpenerStoreError> {
    let mut registration = HandlerRegistration {
        package: String::new(),
        process: String::new(),
        schemes: Vec::new(),
        domains: Vec::new(),
        mime_types: Vec::new(),
        interfaces: Vec::new(),
        domains_verified: false,
    };
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => registration.package = field.string()?,
            2 => registration.process = field.string()?,
            3 => registration.schemes.push(field.string()?),
            4 => registration.domains.push(field.string()?),
            5 => registration.mime_types.push(field.string()?),
            6 => registration.interfaces.push(field.string()?),
            7 => registration.domains_verified = field.varint()? != 0,
            _ => {}
        }
    }
    validate_registration(&registration)?;
    Ok(registration)
}

#[cfg(feature = "redb_backend")]
fn encode_default(uid: u64, key: &str, handler: &HandlerId) -> Vec<u8> {
    let mut out = Vec::new();
    put_varint(&mut out, 1, uid);
    put_string(&mut out, 2, key);
    put_string(&mut out, 3, &handler.package);
    put_string(&mut out, 4, &handler.process);
    out
}

#[cfg(feature = "redb_backend")]
fn decode_default(bytes: &[u8]) -> Result<UserDefault, OpenerStoreError> {
    let mut default = UserDefault {
        uid: 0,
        key: String::new(),
        handler: HandlerId::new("", ""),
    };
    let mut cursor = Cursor::new(bytes);
    while let Some(field) = cursor.next_field()? {
        match field.number {
            1 => default.uid = field.varint()?,
            2 => default.key = field.string()?,
            3 => default.handler.package = field.string()?,
            4 => default.handler.process = field.string()?,
            _ => {}
        }
    }
    validate_default_key(&default.key)?;
    validate_registration(&HandlerRegistration {
        package: default.handler.package.clone(),
        process: default.handler.process.clone(),
        schemes: Vec::new(),
        domains: Vec::new(),
        mime_types: Vec::new(),
        interfaces: Vec::new(),
        domains_verified: false,
    })?;
    Ok(default)
}

#[cfg(feature = "redb_backend")]
pub mod persistent {
    use super::{
        HandlerId, HandlerRegistration, MemoryOpenerRegistry, OpenerScope, OpenerStoreError,
        UserDefault, decode_default, decode_registration, encode_default, encode_registration,
    };
    use alloc::{format, vec::Vec};
    use bexos_redb::{BlockStore, open_or_create_with_store};
    use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};

    const SYSTEM_HANDLERS: TableDefinition<&str, &[u8]> = TableDefinition::new("system_handlers");
    const USER_HANDLERS: TableDefinition<&str, &[u8]> = TableDefinition::new("user_handlers");
    const USER_DEFAULTS: TableDefinition<&str, &[u8]> = TableDefinition::new("user_defaults");

    pub struct OpenerStoreDb {
        db: Database,
    }

    impl OpenerStoreDb {
        pub fn open<S: BlockStore>(store: S) -> Result<Self, OpenerStoreError> {
            Ok(Self {
                db: open_or_create_with_store(store).map_err(|_| OpenerStoreError::Storage)?,
            })
        }

        pub fn register(
            &self,
            scope: OpenerScope,
            registration: HandlerRegistration,
        ) -> Result<(), OpenerStoreError> {
            let mut memory = self.snapshot_memory()?;
            memory.register(scope, registration.clone())?;
            let tx = self
                .db
                .begin_write()
                .map_err(|_| OpenerStoreError::Storage)?;
            {
                match scope {
                    OpenerScope::System => {
                        let mut table = tx
                            .open_table(SYSTEM_HANDLERS)
                            .map_err(|_| OpenerStoreError::Storage)?;
                        let key = registration_key(&registration);
                        let bytes = encode_registration(&registration);
                        table
                            .insert(key.as_str(), bytes.as_slice())
                            .map_err(|_| OpenerStoreError::Storage)?;
                    }
                    OpenerScope::User(uid) => {
                        let mut table = tx
                            .open_table(USER_HANDLERS)
                            .map_err(|_| OpenerStoreError::Storage)?;
                        let key = user_registration_key(uid, &registration);
                        let bytes = encode_registration(&registration);
                        table
                            .insert(key.as_str(), bytes.as_slice())
                            .map_err(|_| OpenerStoreError::Storage)?;
                    }
                }
            }
            tx.commit().map_err(|_| OpenerStoreError::Storage)
        }

        pub fn set_user_default(
            &self,
            uid: u64,
            key: &str,
            handler: HandlerId,
        ) -> Result<(), OpenerStoreError> {
            let mut memory = self.snapshot_memory()?;
            memory.set_user_default(uid, key, handler.clone())?;
            let tx = self
                .db
                .begin_write()
                .map_err(|_| OpenerStoreError::Storage)?;
            {
                let mut table = tx
                    .open_table(USER_DEFAULTS)
                    .map_err(|_| OpenerStoreError::Storage)?;
                let row = user_default_key(uid, key);
                let bytes = encode_default(uid, key, &handler);
                table
                    .insert(row.as_str(), bytes.as_slice())
                    .map_err(|_| OpenerStoreError::Storage)?;
            }
            tx.commit().map_err(|_| OpenerStoreError::Storage)
        }

        pub fn remove_package(&self, package: &str) -> Result<(), OpenerStoreError> {
            let mut memory = self.snapshot_memory()?;
            memory.remove_package(package);
            self.replace_from_memory(&memory)
        }

        pub fn snapshot_memory(&self) -> Result<MemoryOpenerRegistry, OpenerStoreError> {
            let mut registry = MemoryOpenerRegistry::new();
            for registration in self.read_registrations(SYSTEM_HANDLERS)? {
                registry.register(OpenerScope::System, registration)?;
            }
            for (uid, registration) in self.read_user_registrations()? {
                registry.register(OpenerScope::User(uid), registration)?;
            }
            for default in self.read_defaults()? {
                registry.set_user_default(default.uid, default.key, default.handler)?;
            }
            Ok(registry)
        }

        pub fn replace_from_memory(
            &self,
            memory: &MemoryOpenerRegistry,
        ) -> Result<(), OpenerStoreError> {
            let tx = self
                .db
                .begin_write()
                .map_err(|_| OpenerStoreError::Storage)?;
            {
                let mut system = tx
                    .open_table(SYSTEM_HANDLERS)
                    .map_err(|_| OpenerStoreError::Storage)?;
                system
                    .retain(|_, _| false)
                    .map_err(|_| OpenerStoreError::Storage)?;
                for registration in &memory.system {
                    let key = registration_key(registration);
                    let bytes = encode_registration(registration);
                    system
                        .insert(key.as_str(), bytes.as_slice())
                        .map_err(|_| OpenerStoreError::Storage)?;
                }
            }
            {
                let mut users = tx
                    .open_table(USER_HANDLERS)
                    .map_err(|_| OpenerStoreError::Storage)?;
                users
                    .retain(|_, _| false)
                    .map_err(|_| OpenerStoreError::Storage)?;
                for entry in &memory.users {
                    let key = user_registration_key(entry.uid, &entry.registration);
                    let bytes = encode_registration(&entry.registration);
                    users
                        .insert(key.as_str(), bytes.as_slice())
                        .map_err(|_| OpenerStoreError::Storage)?;
                }
            }
            {
                let mut defaults = tx
                    .open_table(USER_DEFAULTS)
                    .map_err(|_| OpenerStoreError::Storage)?;
                defaults
                    .retain(|_, _| false)
                    .map_err(|_| OpenerStoreError::Storage)?;
                for entry in &memory.defaults {
                    let key = user_default_key(entry.uid, &entry.key);
                    let bytes = encode_default(entry.uid, &entry.key, &entry.handler);
                    defaults
                        .insert(key.as_str(), bytes.as_slice())
                        .map_err(|_| OpenerStoreError::Storage)?;
                }
            }
            tx.commit().map_err(|_| OpenerStoreError::Storage)
        }

        fn read_registrations(
            &self,
            table: TableDefinition<&str, &[u8]>,
        ) -> Result<Vec<HandlerRegistration>, OpenerStoreError> {
            let tx = self
                .db
                .begin_read()
                .map_err(|_| OpenerStoreError::Storage)?;
            let Ok(table) = tx.open_table(table) else {
                return Ok(Vec::new());
            };
            table
                .iter()
                .map_err(|_| OpenerStoreError::Storage)?
                .map(|entry| {
                    let (_, value) = entry.map_err(|_| OpenerStoreError::Storage)?;
                    decode_registration(value.value())
                })
                .collect()
        }

        fn read_user_registrations(
            &self,
        ) -> Result<Vec<(u64, HandlerRegistration)>, OpenerStoreError> {
            let tx = self
                .db
                .begin_read()
                .map_err(|_| OpenerStoreError::Storage)?;
            let Ok(table) = tx.open_table(USER_HANDLERS) else {
                return Ok(Vec::new());
            };
            table
                .iter()
                .map_err(|_| OpenerStoreError::Storage)?
                .map(|entry| {
                    let (key, value) = entry.map_err(|_| OpenerStoreError::Storage)?;
                    let uid = key
                        .value()
                        .split_once(':')
                        .and_then(|(uid, _)| uid.parse::<u64>().ok())
                        .ok_or(OpenerStoreError::CorruptRecord)?;
                    Ok((uid, decode_registration(value.value())?))
                })
                .collect()
        }

        fn read_defaults(&self) -> Result<Vec<UserDefault>, OpenerStoreError> {
            let tx = self
                .db
                .begin_read()
                .map_err(|_| OpenerStoreError::Storage)?;
            let Ok(table) = tx.open_table(USER_DEFAULTS) else {
                return Ok(Vec::new());
            };
            table
                .iter()
                .map_err(|_| OpenerStoreError::Storage)?
                .map(|entry| {
                    let (_, value) = entry.map_err(|_| OpenerStoreError::Storage)?;
                    decode_default(value.value())
                })
                .collect()
        }
    }

    fn registration_key(registration: &HandlerRegistration) -> String {
        HandlerId::new(&registration.package, &registration.process).key()
    }

    fn user_registration_key(uid: u64, registration: &HandlerRegistration) -> String {
        format!("{uid}:{}", registration_key(registration))
    }

    fn user_default_key(uid: u64, key: &str) -> String {
        format!("{uid}:{key}")
    }
}

#[cfg(feature = "redb_backend")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Field<'a> {
    number: u32,
    wire_type: u8,
    value: &'a [u8],
}

#[cfg(feature = "redb_backend")]
impl<'a> Field<'a> {
    fn string(self) -> Result<String, OpenerStoreError> {
        if self.wire_type != 2 {
            return Err(OpenerStoreError::CorruptRecord);
        }
        let value =
            core::str::from_utf8(self.value).map_err(|_| OpenerStoreError::CorruptRecord)?;
        Ok(value.to_string())
    }

    fn varint(self) -> Result<u64, OpenerStoreError> {
        if self.wire_type != 0 {
            return Err(OpenerStoreError::CorruptRecord);
        }
        let mut value = 0u64;
        for (shift, byte) in self.value.iter().enumerate() {
            value |= u64::from(byte & 0x7f) << (shift * 7);
        }
        Ok(value)
    }
}

#[cfg(feature = "redb_backend")]
struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

#[cfg(feature = "redb_backend")]
impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn next_field(&mut self) -> Result<Option<Field<'a>>, OpenerStoreError> {
        if self.offset == self.bytes.len() {
            return Ok(None);
        }
        let key = self.read_varint()?;
        let number = (key >> 3) as u32;
        let wire_type = (key & 0x7) as u8;
        match wire_type {
            0 => {
                let start = self.offset;
                self.read_varint()?;
                Ok(Some(Field {
                    number,
                    wire_type,
                    value: &self.bytes[start..self.offset],
                }))
            }
            2 => {
                let len = self.read_varint()? as usize;
                let start = self.offset;
                let end = start
                    .checked_add(len)
                    .ok_or(OpenerStoreError::CorruptRecord)?;
                if end > self.bytes.len() {
                    return Err(OpenerStoreError::CorruptRecord);
                }
                self.offset = end;
                Ok(Some(Field {
                    number,
                    wire_type,
                    value: &self.bytes[start..end],
                }))
            }
            _ => Err(OpenerStoreError::CorruptRecord),
        }
    }

    fn read_varint(&mut self) -> Result<u64, OpenerStoreError> {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = *self
                .bytes
                .get(self.offset)
                .ok_or(OpenerStoreError::CorruptRecord)?;
            self.offset += 1;
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(OpenerStoreError::CorruptRecord)
    }
}

#[cfg(feature = "redb_backend")]
fn put_string(out: &mut Vec<u8>, field: u32, value: &str) {
    put_raw_varint(out, u64::from(field << 3 | 2));
    put_raw_varint(out, value.len() as u64);
    out.extend_from_slice(value.as_bytes());
}

#[cfg(feature = "redb_backend")]
fn put_varint(out: &mut Vec<u8>, field: u32, value: u64) {
    put_raw_varint(out, u64::from(field << 3));
    put_raw_varint(out, value);
}

#[cfg(feature = "redb_backend")]
fn put_raw_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}
