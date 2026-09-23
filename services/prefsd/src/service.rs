//! Policy and transaction state independent of guest transport and filesystem.
use alloc::{
    collections::{BTreeMap, BTreeSet},
    string::String,
    vec::Vec,
};
use bexos_component_config::{
    schema::{Assignments, Error, Schema},
    transaction::{Phase, PreparedSnapshot, Transaction},
};

pub const MANAGE_PERMISSION: &str = "bexos.permission.MANAGE_USER_PREFERENCES";
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Package {
    pub key: String,
    pub id: String,
    pub schema: Schema,
    pub base: Assignments,
    pub product_locks: BTreeSet<String>,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Operator {
    pub generation: u64,
    pub values: Assignments,
    pub locks: BTreeSet<String>,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Preferences {
    pub generation: u64,
    pub values: Assignments,
    pub revisions: BTreeMap<String, u64>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Observer {
    pub channel: u64,
    pub package: String,
    pub uid: u64,
    pub registered: bool,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThemeObserver {
    pub channel: u64,
    pub uid: u64,
    pub generation: u64,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Mutation {
    User {
        uid: u64,
        package: String,
        fingerprint: u64,
        values: Assignments,
    },
    Operator {
        package: String,
        values: Assignments,
        locks: BTreeSet<String>,
    },
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Pending {
    pub delivery_failed: bool,
    pub transaction: Transaction,
    pub mutation: Mutation,
    pub snapshots: Vec<PreparedSnapshot>,
    pub reply_channel: u64,
    pub reply_ordinal: u64,
    pub response_generation: u64,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Service {
    pub frozen: bool,
    pub packages: BTreeMap<String, Package>,
    pub operators: BTreeMap<String, Operator>,
    pub preferences: BTreeMap<(u64, String, u64), Preferences>,
    pub observers: Vec<Observer>,
    pub theme_observers: Vec<ThemeObserver>,
    pub locale_observers: Vec<ThemeObserver>,
    pub locked: BTreeSet<u64>,
    pub pending: Option<Pending>,
    pub sequence: u64,
}
impl Service {
    pub fn register(&mut self, package: Package) -> Result<(), Error> {
        if package.key.is_empty() || package.key.len() > 128 || !valid_package(&package.id) {
            return Err(Error::InvalidSchema);
        }
        package
            .schema
            .validate()
            .map_err(|_| Error::InvalidSchema)?;
        package
            .schema
            .validate_assignments(&package.base, false, &BTreeSet::new())?;
        package.schema.validate_locks(&package.product_locks)?;
        if self.pending.is_some() || self.frozen {
            return Err(Error::Busy);
        }
        if let Some(old) = self.packages.get(&package.key) {
            if old != &package {
                return Err(Error::Conflict);
            }
        }
        self.packages.insert(package.key.clone(), package);
        Ok(())
    }
    pub fn package(&self, selector: &str) -> Result<&Package, Error> {
        if let Some(p) = self.packages.get(selector) {
            return Ok(p);
        }
        let mut matches = self.packages.values().filter(|p| p.id == selector);
        let p = matches.next().ok_or(Error::UnknownField)?;
        if matches.next().is_some() {
            return Err(Error::Conflict);
        }
        Ok(p)
    }
    pub fn authorize(
        &self,
        caller: &str,
        caller_uid: u64,
        manage: bool,
        package: &str,
        uid: u64,
    ) -> Result<(), Error> {
        if self.locked.contains(&uid) {
            return Err(Error::AccessDenied);
        }
        let p = self.package(package)?;
        if !manage && (p.id != caller || uid != caller_uid) {
            return Err(Error::AccessDenied);
        }
        Ok(())
    }
    pub fn locks(&self, p: &Package) -> BTreeSet<String> {
        let mut l = p.product_locks.clone();
        if let Some(o) = self.operators.get(&p.key) {
            l.extend(o.locks.clone());
        }
        l
    }
    pub fn prefs_key(p: &Package, uid: u64) -> (u64, String, u64) {
        (uid, p.id.clone(), p.schema.fingerprint())
    }
    pub fn effective(&self, key: &str, uid: u64) -> Result<(u64, Vec<u8>), Error> {
        if self.locked.contains(&uid) {
            return Err(Error::AccessDenied);
        }
        let p = self.package(key)?;
        let empty = Assignments::new();
        let prefs = self.preferences.get(&Self::prefs_key(p, uid));
        let op = self.operators.get(&p.key);
        let generation = prefs.map_or(0, |p| p.generation);
        let values = p.schema.resolve(
            &p.base,
            op.map_or(&empty, |o| &o.values),
            prefs.map_or(&empty, |p| &p.values),
            &self.locks(p),
        )?;
        Ok((generation, p.schema.encode_table(&values, generation)?))
    }
    pub fn begin_user(
        &mut self,
        key: &str,
        uid: u64,
        expected: u64,
        assignments: Assignments,
        reset: bool,
        now: u64,
        timeout: u64,
    ) -> Result<(), Error> {
        if self.pending.is_some() || self.frozen {
            return Err(Error::Busy);
        }
        if self.locked.contains(&uid) {
            return Err(Error::AccessDenied);
        }
        let p = self.package(key)?.clone();
        p.schema
            .validate_assignments(&assignments, true, &self.locks(&p))?;
        let prefs_key = Self::prefs_key(&p, uid);
        let mut prefs = self
            .preferences
            .get(&prefs_key)
            .cloned()
            .unwrap_or_default();
        if prefs.generation != expected {
            return Err(Error::Conflict);
        }
        if reset {
            prefs.values.clear();
        } else {
            prefs.values.extend(assignments.clone());
        }
        // A shared preference must be writable under every installed version's policy.
        for sibling in self.packages.values().filter(|other| {
            other.id == p.id && other.schema.fingerprint() == p.schema.fingerprint()
        }) {
            if !reset {
                sibling
                    .schema
                    .validate_assignments(&assignments, true, &self.locks(sibling))?;
            }
        }
        let mutation = Mutation::User {
            uid,
            package: p.id,
            fingerprint: p.schema.fingerprint(),
            values: prefs.values,
        };
        self.begin(mutation, expected, now, timeout)
    }
    pub fn begin_operator(
        &mut self,
        key: &str,
        expected: u64,
        values: Assignments,
        names: BTreeSet<String>,
        operation: u32,
        now: u64,
        timeout: u64,
    ) -> Result<(), Error> {
        if self.pending.is_some() || self.frozen {
            return Err(Error::Busy);
        }
        let p = self.package(key)?.clone();
        let mut operator = self.operators.get(&p.key).cloned().unwrap_or_default();
        if operator.generation != expected {
            return Err(Error::Conflict);
        }
        match operation {
            1 => {
                p.schema
                    .validate_assignments(&values, false, &BTreeSet::new())?;
                operator.values.extend(values);
            }
            2 => {
                operator.values.clear();
                operator.locks.clear();
            }
            3 => {
                p.schema.validate_locks(&names)?;
                operator.locks.extend(names);
            }
            4 => {
                p.schema.validate_locks(&names)?;
                if names.iter().any(|n| p.product_locks.contains(n)) {
                    return Err(Error::AccessDenied);
                }
                for name in names {
                    operator.locks.remove(&name);
                }
            }
            _ => return Err(Error::Malformed),
        }
        self.begin(
            Mutation::Operator {
                package: p.key,
                values: operator.values,
                locks: operator.locks,
            },
            expected,
            now,
            timeout,
        )
    }
    fn begin(
        &mut self,
        mutation: Mutation,
        expected: u64,
        now: u64,
        timeout: u64,
    ) -> Result<(), Error> {
        let response_generation = expected.checked_add(1).ok_or(Error::Overflow)?;
        let mut candidate = self.clone();
        candidate.apply(&mutation)?;
        crate::locale::validate_mutation(&candidate, &mutation)?;
        let mut participants = BTreeSet::new();
        let mut snapshots = Vec::new();
        for o in self.observers.iter().filter(|o| o.registered) {
            if self.affected(&mutation, &o.package, o.uid) {
                let (_, bytes) = candidate.effective(&o.package, o.uid)?;
                participants.insert(o.channel);
                snapshots.push(PreparedSnapshot {
                    receiver: o.channel,
                    bytes,
                });
            }
        }
        // Transaction IDs and config generations are separate: one transaction can
        // update different exact-version snapshots with different generations.
        let transaction =
            Transaction::new(self.sequence, self.sequence, now, timeout, participants)?;
        self.sequence = transaction.generation;
        self.pending = Some(Pending {
            delivery_failed: false,
            transaction,
            mutation,
            snapshots,
            reply_channel: 0,
            reply_ordinal: 0,
            response_generation,
        });
        Ok(())
    }
    pub fn affected(&self, m: &Mutation, key: &str, uid: u64) -> bool {
        match m {
            Mutation::Operator { package, .. } => package == key,
            Mutation::User {
                uid: u,
                package,
                fingerprint,
                ..
            } => {
                *u == uid
                    && self
                        .packages
                        .get(key)
                        .is_some_and(|p| p.id == *package && p.schema.fingerprint() == *fingerprint)
            }
        }
    }
    pub fn apply(&mut self, m: &Mutation) -> Result<(), Error> {
        match m {
            Mutation::User {
                uid,
                package,
                fingerprint,
                values,
            } => {
                let prefs = self
                    .preferences
                    .entry((*uid, package.clone(), *fingerprint))
                    .or_default();
                prefs.generation = prefs.generation.checked_add(1).ok_or(Error::Overflow)?;
                prefs.values = values.clone();
            }
            Mutation::Operator {
                package,
                values,
                locks,
            } => {
                let op = self.operators.entry(package.clone()).or_default();
                op.generation = op.generation.checked_add(1).ok_or(Error::Overflow)?;
                op.values = values.clone();
                op.locks = locks.clone();
                let revision = op.generation;
                let p = self.packages.get(package).ok_or(Error::UnknownField)?;
                for ((_, id, fp), prefs) in &mut self.preferences {
                    if *id == p.id && *fp == p.schema.fingerprint() {
                        prefs.generation =
                            prefs.generation.checked_add(1).ok_or(Error::Overflow)?;
                        prefs.revisions.insert(package.clone(), revision);
                    }
                }
            }
        }
        Ok(())
    }
    pub fn candidate(&self) -> Result<Self, Error> {
        let p = self.pending.as_ref().ok_or(Error::Busy)?;
        if p.transaction.phase != Phase::Prepared {
            return Err(Error::Busy);
        }
        let mut candidate = self.clone();
        candidate.apply(&p.mutation)?;
        candidate.sequence = p.transaction.generation;
        Ok(candidate)
    }
    pub fn ensure_user(&mut self, key: &str, uid: u64) -> Result<bool, Error> {
        if self.pending.is_some() || self.frozen {
            return Err(Error::Busy);
        }
        let p = self.package(key)?.clone();
        let op = self.operators.get(&p.key).map_or(0, |o| o.generation);
        let prefs = self
            .preferences
            .entry(Self::prefs_key(&p, uid))
            .or_default();
        let previous = prefs.revisions.get(&p.key).copied().unwrap_or(0);
        if previous != op {
            let missed = op.checked_sub(previous).ok_or(Error::Storage)?;
            prefs.generation = prefs
                .generation
                .checked_add(missed)
                .ok_or(Error::Overflow)?;
            prefs.revisions.insert(p.key, op);
            return Ok(true);
        }
        Ok(false)
    }
    pub fn lock_user(&mut self, uid: u64) {
        self.locked.insert(uid);
        self.preferences.retain(|(u, _, _), _| *u != uid);
        self.observers.retain(|o| o.uid != uid);
    }
}
pub fn valid_package(id: &str) -> bool {
    !id.is_empty()
        && !matches!(id, "." | "..")
        && id.len() <= 128
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'.' || c == b'_' || c == b'-')
}
