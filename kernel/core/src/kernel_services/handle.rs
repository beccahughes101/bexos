use super::{
    Handle, KernelServiceStatus, SIGNAL_PEER_CLOSED, SIGNAL_READABLE, SIGNAL_TERMINATED,
    SIGNAL_WRITABLE,
};
use crate::ipc::Endpoint;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObjectKind {
    Channel,
    Vmo,
    Thread,
    Profile,
    ResourceGroup,
    Process,
    VmSpace,
    Vmar,
    Interrupt,
    ReplyToken,
    GpuReservation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HandleRecord {
    pub handle_id: u64,
    pub object_id: u64,
    pub kind: ObjectKind,
    pub rights: u32,
    pub signals: u32,
    pub owner_task_id: u64,
    pub endpoint: Option<Endpoint>,
}

impl HandleRecord {
    pub const fn new(
        handle_id: u64,
        object_id: u64,
        kind: ObjectKind,
        rights: u32,
        owner_task_id: u64,
        endpoint: Option<Endpoint>,
    ) -> Self {
        Self {
            handle_id,
            object_id,
            kind,
            rights,
            signals: initial_signals(kind),
            owner_task_id,
            endpoint,
        }
    }

    pub const fn has_rights(&self, required: u32) -> bool {
        self.rights & required == required
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HandleTable {
    entries: Vec<HandleRecord>,
    next_handle_id: u64,
}

impl HandleTable {
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
            next_handle_id: 1,
        }
    }

    pub fn insert(
        &mut self,
        object_id: u64,
        kind: ObjectKind,
        rights: u32,
        owner_task_id: u64,
        endpoint: Option<Endpoint>,
    ) -> Result<Handle, KernelServiceStatus> {
        self.entries
            .try_reserve(1)
            .map_err(|_| KernelServiceStatus::NoMemory)?;
        let handle_id = self.next_handle_id;
        self.next_handle_id = self.next_handle_id.saturating_add(1);
        self.entries.push(HandleRecord::new(
            handle_id,
            object_id,
            kind,
            rights,
            owner_task_id,
            endpoint,
        ));
        Ok(Handle { raw: handle_id })
    }

    pub fn get(&self, raw: u64) -> Option<HandleRecord> {
        self.entries.iter().find_map(|record| {
            if record.handle_id == raw {
                Some(*record)
            } else {
                None
            }
        })
    }

    pub fn get_mut(&mut self, raw: u64) -> Option<&mut HandleRecord> {
        self.entries
            .iter_mut()
            .find(|record| record.handle_id == raw)
    }

    pub fn entry_for_object(&self, object_id: u64, kind: ObjectKind) -> Option<HandleRecord> {
        self.entries
            .iter()
            .find(|record| record.object_id == object_id && record.kind == kind)
            .copied()
    }

    pub fn handles_for_object(&self, object_id: u64, kind: ObjectKind) -> alloc::vec::Vec<u64> {
        self.entries
            .iter()
            .filter(|record| record.object_id == object_id && record.kind == kind)
            .map(|record| record.handle_id)
            .collect()
    }

    pub fn set_signals(&mut self, raw: u64, signals: u32) {
        if let Some(record) = self.get_mut(raw) {
            record.signals = signals;
        }
    }

    pub fn add_signals_for_object(&mut self, object_id: u64, kind: ObjectKind, signals: u32) {
        for record in &mut self.entries {
            if record.object_id == object_id && record.kind == kind {
                record.signals |= signals;
            }
        }
    }

    pub fn remove_signals_for_object(&mut self, object_id: u64, kind: ObjectKind, signals: u32) {
        for record in &mut self.entries {
            if record.object_id == object_id && record.kind == kind {
                record.signals &= !signals;
            }
        }
    }

    pub fn remove(&mut self, raw: u64) -> Option<HandleRecord> {
        let index = self
            .entries
            .iter()
            .position(|record| record.handle_id == raw)?;
        Some(self.entries.swap_remove(index))
    }

    pub fn remove_for_owner(&mut self, owner_task_id: u64) -> Option<HandleRecord> {
        let index = self
            .entries
            .iter()
            .position(|record| record.owner_task_id == owner_task_id)?;
        Some(self.entries.swap_remove(index))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

impl Default for HandleTable {
    fn default() -> Self {
        Self::new()
    }
}

const fn initial_signals(kind: ObjectKind) -> u32 {
    match kind {
        ObjectKind::Channel => SIGNAL_WRITABLE,
        ObjectKind::Vmo
        | ObjectKind::Profile
        | ObjectKind::ResourceGroup
        | ObjectKind::Process
        | ObjectKind::VmSpace
        | ObjectKind::Vmar
        | ObjectKind::Interrupt => SIGNAL_SIGNALED_OR_WRITABLE,
        ObjectKind::ReplyToken | ObjectKind::GpuReservation => SIGNAL_SIGNALED_OR_WRITABLE,
        ObjectKind::Thread => 0,
    }
}

const SIGNAL_SIGNALED_OR_WRITABLE: u32 = SIGNAL_WRITABLE;

pub const fn closed_signals(kind: ObjectKind) -> u32 {
    match kind {
        ObjectKind::Channel => SIGNAL_PEER_CLOSED,
        ObjectKind::Thread => SIGNAL_TERMINATED,
        _ => SIGNAL_READABLE,
    }
}
use alloc::vec::Vec;
