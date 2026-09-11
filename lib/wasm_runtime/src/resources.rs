//! Instance-local resource names. Native handles are never guest-visible.
use std::{collections::BTreeMap, sync::Arc};
use wasmtime::{Result, bail};
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    Channel,
    Directory,
    File,
    Socket,
    Opaque,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Origin {
    Signed,
    Unsigned,
}
pub const READ: u32 = 1;
pub const WRITE: u32 = 2;
pub const TRANSFER: u32 = 4;
/// Authenticated grant metadata survives typed child delegation and migration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Grant {
    pub service: String,
    pub protocol: String,
    pub capability: String,
    pub method_ordinals: Vec<u64>,
    pub permission_values: Vec<String>,
    pub caller_package: Option<String>,
    pub caller_uid: Option<u64>,
    pub caller_foreground: bool,
}
/// Trusted platform implementation owns closing and duplicating native handles.
pub trait Handle: Send + Sync {
    fn grant(&self) -> Option<&Grant> {
        None
    }
    fn kind(&self) -> Kind;
    fn rights(&self) -> u32;
    fn native(&self) -> u64;
    fn companions(&self) -> &[u64] {
        &[]
    }
    fn allowed_methods(&self) -> Option<&[u64]> {
        None
    }
}
#[derive(Clone)]
pub struct Entry {
    pub name: String,
    pub handle: Arc<dyn Handle>,
}
pub struct Resources {
    budget: Option<Arc<crate::budget::Budget>>,
    entries: BTreeMap<u32, Entry>,
    next: u32,
    maximum: usize,
    origin: Origin,
}
impl Resources {
    pub fn new(maximum: usize, origin: Origin) -> Self {
        Self {
            budget: None,
            entries: BTreeMap::new(),
            next: 1,
            maximum,
            origin,
        }
    }
    pub fn with_budget(mut self, budget: Arc<crate::budget::Budget>) -> Self {
        self.budget = Some(budget);
        self
    }
    pub fn insert(&mut self, entry: Entry) -> Result<u32> {
        if self.origin == Origin::Unsigned {
            bail!("unsigned code cannot receive external handles");
        }
        if self.entries.len() >= self.maximum {
            bail!("handle limit");
        }
        let id = self.next;
        self.next = self
            .next
            .checked_add(1)
            .ok_or_else(|| wasmtime::format_err!("resource id exhausted"))?;
        if self.budget.as_ref().is_some_and(|b| !b.charge_handles(1)) {
            bail!("aggregate handle limit");
        }
        self.entries.insert(id, entry);
        Ok(id)
    }
    /// Reserve all possible incoming IDs and budget before consuming the kernel
    /// message. A failed receive releases the reservation without changing IDs.
    pub fn receive<T>(
        &mut self,
        maximum: usize,
        receive: impl FnOnce() -> Result<(T, Vec<Arc<dyn Handle>>)>,
    ) -> Result<(T, Vec<u32>)> {
        if self.origin == Origin::Unsigned
            || maximum > self.remaining()
            || self.next.checked_add(maximum as u32).is_none()
        {
            bail!("receive resource limit");
        }
        struct Reservation {
            budget: Option<Arc<crate::budget::Budget>>,
            count: usize,
        }
        impl Drop for Reservation {
            fn drop(&mut self) {
                if let Some(b) = &self.budget {
                    b.release_handles(self.count);
                }
            }
        }
        if self
            .budget
            .as_ref()
            .is_some_and(|b| !b.charge_handles(maximum))
        {
            bail!("aggregate receive handle limit");
        }
        let mut reservation = Reservation {
            budget: self.budget.clone(),
            count: maximum,
        };
        let (data, handles) = receive()?;
        if handles.len() > maximum {
            bail!("host exceeded receive reservation");
        }
        let mut ids = Vec::with_capacity(handles.len());
        for handle in handles {
            let id = self.next;
            self.next += 1;
            self.entries.insert(
                id,
                Entry {
                    name: String::new(),
                    handle,
                },
            );
            ids.push(id);
            reservation.count -= 1;
        }
        Ok((data, ids))
    }
    pub fn get(&self, id: u32, kind: Kind, rights: u32) -> Result<&Entry> {
        let e = self
            .entries
            .get(&id)
            .ok_or_else(|| wasmtime::format_err!("invalid resource"))?;
        if e.handle.kind() != kind || e.handle.rights() & rights != rights {
            bail!("resource rights or type mismatch");
        }
        Ok(e)
    }
    pub fn transferable(&self, id: u32) -> Result<&Entry> {
        let e = self
            .entries
            .get(&id)
            .ok_or_else(|| wasmtime::format_err!("invalid resource"))?;
        if e.handle.rights() & TRANSFER == 0 {
            bail!("resource is not transferable");
        }
        Ok(e)
    }
    pub fn remove(&mut self, id: u32) -> Result<Entry> {
        let entry = self
            .entries
            .remove(&id)
            .ok_or_else(|| wasmtime::format_err!("invalid resource"))?;
        if let Some(b) = &self.budget {
            b.release_handles(1);
        }
        Ok(entry)
    }
    pub fn find(&self, name: &str) -> Option<u32> {
        self.entries
            .iter()
            .find(|(_, e)| e.name == name)
            .map(|(id, _)| *id)
    }
    pub fn entries(&self) -> impl Iterator<Item = (u32, &Entry)> {
        self.entries.iter().map(|(id, e)| (*id, e))
    }
    pub fn remaining(&self) -> usize {
        self.maximum - self.entries.len()
    }
    pub fn next_id(&self) -> u32 {
        self.next
    }
    pub fn restore(&mut self, next: u32, entries: Vec<(u32, Entry)>) -> Result<()> {
        if !self.entries.is_empty()
            || next == 0
            || entries.len() > self.maximum
            || (self.origin == Origin::Unsigned && !entries.is_empty())
        {
            bail!("invalid restored resources");
        }
        let mut restored = BTreeMap::new();
        for (id, e) in entries {
            if id == 0 || id >= next || restored.insert(id, e).is_some() {
                bail!("duplicate or stale restored resource");
            }
        }
        if self
            .budget
            .as_ref()
            .is_some_and(|b| !b.charge_handles(restored.len()))
        {
            bail!("aggregate restored handle limit");
        }
        self.entries = restored;
        self.next = next;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    struct H;
    impl Handle for H {
        fn kind(&self) -> Kind {
            Kind::Channel
        }
        fn rights(&self) -> u32 {
            READ
        }
        fn native(&self) -> u64 {
            123
        }
    }
    fn entry() -> Entry {
        Entry {
            name: "test".into(),
            handle: Arc::new(H),
        }
    }
    #[test]
    fn handles_are_local_typed_and_never_reused() {
        let mut r = Resources::new(1, Origin::Signed);
        let a = r.insert(entry()).unwrap();
        assert!(r.get(a, Kind::Channel, READ).is_ok());
        assert!(r.get(a, Kind::File, READ).is_err());
        assert!(r.get(a, Kind::Channel, WRITE).is_err());
        assert!(r.insert(entry()).is_err());
        r.remove(a).unwrap();
        let b = r.insert(entry()).unwrap();
        assert_ne!(a, b);
        assert!(r.get(a, Kind::Channel, READ).is_err());
        assert!(r.transferable(b).is_err());
    }
    #[test]
    fn unsigned_rejects_handles_at_admission_and_restore() {
        let mut r = Resources::new(2, Origin::Unsigned);
        assert!(r.insert(entry()).is_err());
        assert!(r.restore(2, vec![(1, entry())]).is_err());
    }
}

impl Drop for Resources {
    fn drop(&mut self) {
        if let Some(b) = &self.budget {
            b.release_handles(self.entries.len());
        }
    }
}
