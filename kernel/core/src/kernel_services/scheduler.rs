use super::handle::ObjectKind;
use super::{
    ControlPlane, Handle, HardwareAccess, KernelServiceStatus, RIGHT_MANAGE_TASK, RIGHT_SET_POLICY,
    RIGHT_TRANSFER,
};
use crate::sched::{DeadlineProfile, FairProfile, SchedulerError, SchedulingProfile};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProfileRecord {
    pub id: u64,
    pub owner_process_id: u64,
    pub profile: SchedulingProfile,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProfileTable {
    entries: Vec<ProfileRecord>,
    next_profile_id: u64,
}

impl ProfileTable {
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
            next_profile_id: 1,
        }
    }

    pub fn insert(
        &mut self,
        owner_process_id: u64,
        profile: SchedulingProfile,
    ) -> Result<u64, KernelServiceStatus> {
        if owner_process_id == 0 {
            return Err(KernelServiceStatus::InvalidArgs);
        }
        self.entries
            .try_reserve(1)
            .map_err(|_| KernelServiceStatus::NoMemory)?;
        let id = self.next_profile_id;
        self.next_profile_id = self.next_profile_id.saturating_add(1);
        self.entries.push(ProfileRecord {
            id,
            owner_process_id,
            profile,
        });
        Ok(id)
    }

    pub fn get(&self, id: u64) -> Option<ProfileRecord> {
        self.entries
            .iter()
            .find(|profile| profile.id == id)
            .copied()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

impl Default for ProfileTable {
    fn default() -> Self {
        Self::new()
    }
}

impl ControlPlane {
    pub fn create_scheduling_profile(
        &mut self,
        profile: SchedulingProfile,
    ) -> Result<Handle, KernelServiceStatus> {
        let current = self
            .threads
            .get(self.threads.current_thread_id())
            .ok_or(KernelServiceStatus::InvalidHandle)?;
        let process = self
            .processes
            .get(current.process_id)
            .ok_or(KernelServiceStatus::InvalidHandle)?;
        let profile = validate_profile_request(profile, process.hardware_access)?;
        let id = self.profiles.insert(process.id, profile)?;
        self.handles.insert(
            id,
            ObjectKind::Profile,
            RIGHT_TRANSFER | RIGHT_MANAGE_TASK | RIGHT_SET_POLICY,
            process.id,
            None,
        )
    }

    pub fn profile_for_handle(
        &self,
        profile: Handle,
    ) -> Result<ProfileRecord, KernelServiceStatus> {
        let Some(record) = self.handles.get(profile.raw) else {
            return Err(KernelServiceStatus::InvalidHandle);
        };
        if record.kind != ObjectKind::Profile || !record.has_rights(RIGHT_SET_POLICY) {
            return Err(KernelServiceStatus::AccessDenied);
        }
        self.profiles
            .get(record.object_id)
            .ok_or(KernelServiceStatus::InvalidHandle)
    }
}

fn validate_profile_request(
    profile: SchedulingProfile,
    hardware_access: HardwareAccess,
) -> Result<SchedulingProfile, KernelServiceStatus> {
    match profile {
        SchedulingProfile::Fair(mut fair) => {
            if fair.priority == 0 || fair.weight == 0 {
                return Err(KernelServiceStatus::InvalidArgs);
            }
            if hardware_access != HardwareAccess::Direct && fair.priority > 127 {
                fair.priority = 127;
            }
            Ok(SchedulingProfile::Fair(fair))
        }
        SchedulingProfile::Deadline(deadline) => {
            if hardware_access != HardwareAccess::Direct {
                return Err(KernelServiceStatus::AccessDenied);
            }
            if !deadline.valid() {
                return Err(KernelServiceStatus::InvalidArgs);
            }
            Ok(SchedulingProfile::Deadline(deadline))
        }
    }
}

pub(super) fn scheduler_error(error: SchedulerError) -> KernelServiceStatus {
    match error {
        SchedulerError::Full => KernelServiceStatus::NoMemory,
        SchedulerError::InvalidTask => KernelServiceStatus::InvalidArgs,
        SchedulerError::AlreadyExists => KernelServiceStatus::AlreadyExists,
        SchedulerError::AccessDenied => KernelServiceStatus::AccessDenied,
        SchedulerError::ResourceExhausted => KernelServiceStatus::ResourceExhausted,
    }
}

pub const DEFAULT_FAIR_PROFILE: FairProfile = FairProfile {
    priority: 1,
    weight: 1,
};

pub const DEFAULT_DEADLINE_PROFILE: DeadlineProfile = DeadlineProfile {
    capacity_ns: 1,
    deadline_ns: 1,
    period_ns: 1,
};

use alloc::vec::Vec;
