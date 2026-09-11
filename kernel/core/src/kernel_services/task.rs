use super::handle::ObjectKind;
use super::scheduler::scheduler_error;
use super::system::RESOURCE_GROUP_SYSTEM;
use super::{
    ControlPlane, Handle, KernelServiceStatus, RIGHT_MANAGE_TASK, RIGHT_READ, RIGHT_SIGNAL,
    RIGHT_TRANSFER, RIGHT_WRITE, SIGNAL_TERMINATED, WaitManyItem, WaitManyResult, WakeResult,
};
use crate::sched::{BlockReason, MAX_WAIT_MANY_ITEMS, SchedulerTask, SchedulingProfile};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThreadState {
    Ready,
    Running,
    Blocked,
    Suspended,
    Exited,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ThreadRecord {
    pub id: u64,
    pub process_id: u64,
    pub vm_space_id: u64,
    pub entry_vaddr: u64,
    pub stack_top_vaddr: u64,
    pub arg_handle: Option<Handle>,
    pub state: ThreadState,
    pub priority: u8,
    pub base_priority: u8,
    pub effective_priority: u8,
    pub cpu_affinity_mask: u64,
    pub profile: Option<SchedulingProfile>,
    pub exit_code: i32,
    pub saved_sp_el0: u64,
    pub saved_elr_el1: u64,
    pub saved_spsr_el1: u64,
    pub saved_tpidr_el0: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThreadTable {
    entries: Vec<ThreadRecord>,
    next_thread_id: u64,
    current_thread_id: u64,
}

impl ThreadTable {
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
            next_thread_id: 1,
            current_thread_id: 0,
        }
    }

    pub fn insert(
        &mut self,
        process_id: u64,
        vm_space_id: u64,
        entry_vaddr: u64,
        stack_top_vaddr: u64,
        thread_pointer_vaddr: u64,
        arg_handle: Option<Handle>,
        priority: u8,
    ) -> Result<u64, KernelServiceStatus> {
        if process_id == 0 || vm_space_id == 0 || priority == 0 {
            return Err(KernelServiceStatus::InvalidArgs);
        }
        self.entries
            .try_reserve(1)
            .map_err(|_| KernelServiceStatus::NoMemory)?;
        let id = self.next_thread_id;
        self.next_thread_id = self.next_thread_id.saturating_add(1);
        if self.current_thread_id == 0 {
            self.current_thread_id = id;
        }
        self.entries.push(ThreadRecord {
            id,
            process_id,
            vm_space_id,
            entry_vaddr,
            stack_top_vaddr,
            arg_handle,
            state: ThreadState::Ready,
            priority,
            base_priority: priority,
            effective_priority: priority,
            cpu_affinity_mask: 1,
            profile: None,
            exit_code: 0,
            saved_sp_el0: stack_top_vaddr,
            saved_elr_el1: entry_vaddr,
            saved_spsr_el1: 0,
            saved_tpidr_el0: thread_pointer_vaddr,
        });
        Ok(id)
    }

    pub fn get(&self, id: u64) -> Option<ThreadRecord> {
        self.entries.iter().find(|thread| thread.id == id).copied()
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut ThreadRecord> {
        self.entries.iter_mut().find(|thread| thread.id == id)
    }

    pub fn exit_current(&mut self, exit_code: i32) -> Option<u64> {
        let current = self.current_thread_id;
        let record = self.get_mut(current)?;
        record.state = ThreadState::Exited;
        record.exit_code = exit_code;
        Some(current)
    }

    pub fn exit_process(&mut self, process_id: u64, exit_code: i32) -> Vec<u64> {
        let mut exited = Vec::new();
        for thread in &mut self.entries {
            if thread.process_id == process_id && thread.state != ThreadState::Exited {
                thread.state = ThreadState::Exited;
                thread.exit_code = exit_code;
                let _ = exited.try_reserve(1);
                exited.push(thread.id);
            }
        }
        exited
    }

    pub fn set_current(&mut self, id: u64) -> Result<(), KernelServiceStatus> {
        if self.get(id).is_none() {
            return Err(KernelServiceStatus::InvalidHandle);
        }
        self.current_thread_id = id;
        for thread in &mut self.entries {
            if thread.state != ThreadState::Exited {
                thread.state = if thread.id == id {
                    ThreadState::Running
                } else if thread.state == ThreadState::Running {
                    ThreadState::Ready
                } else {
                    thread.state
                };
            }
        }
        Ok(())
    }

    pub const fn current_thread_id(&self) -> u64 {
        self.current_thread_id
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn process_has_live_threads(&self, process_id: u64) -> bool {
        self.entries
            .iter()
            .any(|thread| thread.process_id == process_id && thread.state != ThreadState::Exited)
    }
}

impl Default for ThreadTable {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FutexWaiter {
    pub thread_id: u64,
    pub uaddr: u64,
    pub expected_val: u32,
    pub owner_thread_id: Option<u64>,
    pub donated_priority: u8,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FutexTable {
    entries: Vec<FutexWaiter>,
    last_woken: Vec<u64>,
}

impl FutexTable {
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
            last_woken: Vec::new(),
        }
    }

    pub fn wait(
        &mut self,
        thread_id: u64,
        uaddr: u64,
        expected_val: u32,
        owner_thread_id: Option<u64>,
        donated_priority: u8,
    ) -> KernelServiceStatus {
        if self.entries.try_reserve(1).is_err() {
            return KernelServiceStatus::NoMemory;
        }
        self.entries.push(FutexWaiter {
            thread_id,
            uaddr,
            expected_val,
            owner_thread_id,
            donated_priority,
        });
        KernelServiceStatus::Ok
    }

    pub fn wake(&mut self, uaddr: u64, wake_count: u32) -> u32 {
        self.last_woken.clear();
        if self.last_woken.try_reserve(wake_count as usize).is_err() {
            return 0;
        }
        let mut woken = 0u32;
        let mut index = 0;
        while index < self.entries.len() {
            if woken == wake_count {
                break;
            }
            if self.entries[index].uaddr == uaddr {
                let waiter = self.entries.swap_remove(index);
                self.last_woken.push(waiter.thread_id);
                woken += 1;
            } else {
                index += 1;
            }
        }
        woken
    }

    pub fn last_woken_ids(&self) -> &[u64] {
        &self.last_woken
    }

    pub fn remove_thread(&mut self, thread_id: u64) -> Vec<u64> {
        let mut owners = Vec::new();
        let mut index = 0;
        while index < self.entries.len() {
            if self.entries[index].thread_id == thread_id
                || self.entries[index].owner_thread_id == Some(thread_id)
            {
                if let Some(owner) = self.entries[index].owner_thread_id {
                    let _ = owners.try_reserve(1);
                    owners.push(owner);
                }
                self.entries.swap_remove(index);
            } else {
                index += 1;
            }
        }
        owners
    }

    pub fn max_donation_for_owner(&self, owner_thread_id: u64) -> Option<u8> {
        self.entries
            .iter()
            .filter(|waiter| waiter.owner_thread_id == Some(owner_thread_id))
            .map(|waiter| waiter.donated_priority)
            .max()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

impl Default for FutexTable {
    fn default() -> Self {
        Self::new()
    }
}

impl ControlPlane {
    pub fn create_thread(
        &mut self,
        entry_vaddr: u64,
        stack_top_vaddr: u64,
        arg_handle: Option<Handle>,
    ) -> Result<Handle, KernelServiceStatus> {
        let (process, vm_space) = self.ensure_current_process()?;
        self.start_thread_in_process(process, vm_space, entry_vaddr, stack_top_vaddr, arg_handle)
    }

    pub fn start_thread_in_process(
        &mut self,
        process: Handle,
        vm_space: Handle,
        entry_vaddr: u64,
        stack_top_vaddr: u64,
        arg_handle: Option<Handle>,
    ) -> Result<Handle, KernelServiceStatus> {
        self.start_thread_in_process_with_thread_pointer(
            process,
            vm_space,
            entry_vaddr,
            stack_top_vaddr,
            0,
            arg_handle,
        )
    }

    pub fn start_thread_in_process_with_thread_pointer(
        &mut self,
        process: Handle,
        vm_space: Handle,
        entry_vaddr: u64,
        stack_top_vaddr: u64,
        thread_pointer_vaddr: u64,
        arg_handle: Option<Handle>,
    ) -> Result<Handle, KernelServiceStatus> {
        if entry_vaddr == 0 || stack_top_vaddr == 0 {
            return Err(KernelServiceStatus::InvalidArgs);
        }
        if let Some(handle) = arg_handle {
            if self.handles.get(handle.raw).is_none() {
                return Err(KernelServiceStatus::InvalidHandle);
            }
        }
        let process_record = self.process_for_handle(process)?;
        let vm_space_record = self.vm_space_for_handle(vm_space)?;
        if vm_space_record.process_id != process_record.id
            || vm_space_record.id != process_record.vm_space_id
        {
            return Err(KernelServiceStatus::InvalidArgs);
        }
        let id = self.threads.insert(
            process_record.id,
            vm_space_record.id,
            entry_vaddr,
            stack_top_vaddr,
            thread_pointer_vaddr,
            arg_handle,
            1,
        )?;
        let active_cpu_mask = self.scheduler.active_cpu_mask();
        if let Some(thread) = self.threads.get_mut(id) {
            thread.cpu_affinity_mask = active_cpu_mask;
        }
        let mut scheduled = SchedulerTask::new(
            id,
            process_record.id,
            process_record.resource_group_id,
            1,
            "thread",
        )
        .map_err(scheduler_error)?;
        scheduled.cpu_affinity_mask = active_cpu_mask;
        self.scheduler
            .add_task(scheduled)
            .map_err(scheduler_error)?;
        if process_record.main_thread_id == 0 {
            let _ = self.processes.set_main_thread(process_record.id, id);
        }
        if let Some(current) = self.scheduler.current() {
            let _ = self.threads.set_current(current.id);
        }
        self.handles.insert(
            id,
            ObjectKind::Thread,
            RIGHT_MANAGE_TASK | RIGHT_SIGNAL | RIGHT_TRANSFER,
            process_record.id,
            None,
        )
    }

    pub fn exit_thread(&mut self, exit_code: i32) -> KernelServiceStatus {
        let process_id = self
            .threads
            .get(self.threads.current_thread_id())
            .map(|thread| thread.process_id);
        let Some(thread_id) = self.threads.exit_current(exit_code) else {
            return KernelServiceStatus::InvalidHandle;
        };
        self.handles
            .add_signals_for_object(thread_id, ObjectKind::Thread, SIGNAL_TERMINATED);
        self.wake_handle_waiters_for_object(thread_id, ObjectKind::Thread, SIGNAL_TERMINATED);
        let _ = self.scheduler.exit_current(exit_code);
        for owner in self.futexes.remove_thread(thread_id) {
            self.restore_inherited_priority(owner);
        }
        if let Some(process_id) = process_id {
            if !self.threads.process_has_live_threads(process_id) {
                let _ = self
                    .processes
                    .set_state(process_id, super::system::ProcessState::Exited);
                self.close_handles_for_process(process_id);
            }
        }
        KernelServiceStatus::Ok
    }

    pub fn futex_wait(
        &mut self,
        uaddr: u64,
        expected_val: u32,
        timeout_nanos: i64,
        owner_thread: Option<Handle>,
    ) -> KernelServiceStatus {
        if uaddr == 0 || uaddr & 0x3 != 0 {
            return KernelServiceStatus::InvalidArgs;
        }
        if timeout_nanos == 0 {
            return KernelServiceStatus::TimedOut;
        }
        let thread_id = self.threads.current_thread_id();
        if thread_id == 0 {
            return KernelServiceStatus::InvalidHandle;
        }
        let owner_thread_id = match owner_thread {
            Some(handle) => match self.thread_id_for_handle(handle) {
                Ok(id) => Some(id),
                Err(status) => return status,
            },
            None => None,
        };
        let donated_priority = self
            .threads
            .get(thread_id)
            .map(|thread| thread.effective_priority)
            .unwrap_or(0);
        if let Some(owner) = owner_thread_id {
            let _ = self.scheduler.donate_priority(owner, donated_priority);
            if let Some(thread) = self.threads.get_mut(owner) {
                if donated_priority > thread.effective_priority {
                    thread.effective_priority = donated_priority;
                    thread.priority = donated_priority;
                }
            }
        }
        let status = self.futexes.wait(
            thread_id,
            uaddr,
            expected_val,
            owner_thread_id,
            donated_priority,
        );
        if status == KernelServiceStatus::Ok {
            if let Some(thread) = self.threads.get_mut(thread_id) {
                thread.state = ThreadState::Blocked;
            }
            if let Ok(decision) = self.scheduler.block_current(BlockReason::Futex { uaddr }) {
                if let Some(next) = decision.next_task_id {
                    let _ = self.threads.set_current(next);
                }
            }
        }
        status
    }

    pub fn futex_wake(&mut self, uaddr: u64, wake_count: u32) -> WakeResult {
        if uaddr == 0 || uaddr & 0x3 != 0 {
            return WakeResult {
                status: KernelServiceStatus::InvalidArgs,
                woken_count: 0,
            };
        }
        let woken = self.futexes.wake(uaddr, wake_count);
        let woken_threads = self.futexes.last_woken_ids().to_vec();
        for thread_id in woken_threads {
            if let Some(thread) = self.threads.get_mut(thread_id) {
                thread.state = ThreadState::Ready;
            }
            let _ = self.scheduler.wake_task(thread_id);
        }
        self.restore_all_inherited_priorities();
        WakeResult {
            status: KernelServiceStatus::Ok,
            woken_count: woken,
        }
    }

    pub fn set_thread_profile(&mut self, thread: Handle, profile: Handle) -> KernelServiceStatus {
        let thread_id = match self.thread_id_for_handle(thread) {
            Ok(thread_id) => thread_id,
            Err(status) => return status,
        };
        let profile = match self.profile_for_handle(profile) {
            Ok(profile) => profile.profile,
            Err(status) => return status,
        };
        if let Err(error) = self.scheduler.set_profile(thread_id, profile) {
            return scheduler_error(error);
        }
        if let Some(thread) = self.threads.get_mut(thread_id) {
            thread.profile = Some(profile);
            match profile {
                SchedulingProfile::Fair(profile) => {
                    thread.priority = profile.priority;
                    thread.base_priority = profile.priority;
                    thread.effective_priority = profile.priority;
                }
                SchedulingProfile::Deadline(_) => {
                    thread.priority = u8::MAX;
                    thread.base_priority = u8::MAX;
                    thread.effective_priority = u8::MAX;
                }
            }
        }
        KernelServiceStatus::Ok
    }

    pub fn set_thread_cpu_affinity(
        &mut self,
        thread: Handle,
        affinity_mask: u64,
    ) -> KernelServiceStatus {
        if affinity_mask == 0 || affinity_mask & !self.scheduler.active_cpu_mask() != 0 {
            return KernelServiceStatus::InvalidArgs;
        }
        let thread_id = match self.thread_id_for_handle(thread) {
            Ok(thread_id) => thread_id,
            Err(status) => return status,
        };
        if let Err(error) = self.scheduler.set_cpu_affinity(thread_id, affinity_mask) {
            return scheduler_error(error);
        }
        if let Some(thread) = self.threads.get_mut(thread_id) {
            thread.cpu_affinity_mask = affinity_mask;
            return KernelServiceStatus::Ok;
        }
        KernelServiceStatus::InvalidHandle
    }

    pub fn yield_thread(&mut self, target_thread: Option<Handle>) -> KernelServiceStatus {
        let target_id = match target_thread {
            Some(handle) => match self.thread_id_for_handle(handle) {
                Ok(id) => Some(id),
                Err(status) => return status,
            },
            None => None,
        };
        let decision = match self.scheduler.yield_current_to(target_id) {
            Ok(decision) => decision,
            Err(error) => return scheduler_error(error),
        };
        if let Some(next) = decision.next_task_id {
            let _ = self.threads.set_current(next);
        }
        KernelServiceStatus::Ok
    }

    fn thread_id_for_handle(&self, thread: Handle) -> Result<u64, KernelServiceStatus> {
        let Some(record) = self.handles.get(thread.raw) else {
            return Err(KernelServiceStatus::InvalidHandle);
        };
        if record.kind != ObjectKind::Thread || !record.has_rights(RIGHT_MANAGE_TASK) {
            return Err(KernelServiceStatus::AccessDenied);
        }
        if self.threads.get(record.object_id).is_none() {
            return Err(KernelServiceStatus::InvalidHandle);
        }
        Ok(record.object_id)
    }

    fn restore_all_inherited_priorities(&mut self) {
        let thread_ids = (0..self.threads.len())
            .filter_map(|index| self.threads.entries.get(index).map(|thread| thread.id))
            .collect::<Vec<_>>();
        for thread_id in thread_ids {
            self.restore_inherited_priority(thread_id);
        }
    }

    pub(super) fn restore_inherited_priority(&mut self, thread_id: u64) {
        let Some(thread) = self.threads.get_mut(thread_id) else {
            return;
        };
        if matches!(thread.profile, Some(SchedulingProfile::Deadline(_))) {
            return;
        }
        let restored = self
            .futexes
            .max_donation_for_owner(thread_id)
            .map_or(thread.base_priority, |donation| {
                donation.max(thread.base_priority)
            });
        thread.effective_priority = restored;
        thread.priority = restored;
        if restored == thread.base_priority {
            let _ = self.scheduler.restore_base_priority(thread_id);
        } else {
            let _ = self.scheduler.donate_priority(thread_id, restored);
        }
    }

    pub fn wait_many(&mut self, items: &[WaitManyItem], deadline_nanos: i64) -> WaitManyResult {
        if items.is_empty() {
            return wait_status(KernelServiceStatus::InvalidArgs);
        }
        for (index, item) in items.iter().enumerate() {
            let Some(record) = self.handles.get(item.handle.raw) else {
                return wait_status(KernelServiceStatus::InvalidHandle);
            };
            if !record.has_rights(RIGHT_SIGNAL) {
                return wait_status(KernelServiceStatus::AccessDenied);
            }
            let observed = record.signals & item.signals;
            if observed != 0 {
                return WaitManyResult {
                    status: KernelServiceStatus::Ok,
                    satisfied_index: index as u32,
                    observed_signals: observed,
                };
            }
        }
        if deadline_nanos != 0 {
            let deadline = if deadline_nanos < 0 {
                u64::MAX
            } else {
                deadline_nanos as u64
            };
            let mut wait_items = [(0, 0); MAX_WAIT_MANY_ITEMS];
            for (slot, item) in wait_items.iter_mut().zip(items.iter()) {
                *slot = (item.handle.raw, item.signals);
            }
            let reason = BlockReason::WaitMany {
                items: wait_items,
                item_count: items.len().min(MAX_WAIT_MANY_ITEMS) as u8,
                deadline_nanos: deadline,
            };
            let _ = self.scheduler.block_current(reason);
        }
        wait_status(KernelServiceStatus::TimedOut)
    }

    pub(super) fn wake_handle_waiters_for_object(
        &mut self,
        object_id: u64,
        kind: ObjectKind,
        signals: u32,
    ) {
        for handle in self.handles.handles_for_object(object_id, kind) {
            let _ = self.scheduler.wake_handle_waiters(handle, signals);
        }
    }

    pub(super) fn ensure_current_process(
        &mut self,
    ) -> Result<(Handle, Handle), KernelServiceStatus> {
        if let Some(current) = self.threads.get(self.threads.current_thread_id()) {
            let process_handle = self.handles.insert(
                current.process_id,
                ObjectKind::Process,
                RIGHT_MANAGE_TASK | RIGHT_TRANSFER,
                current.process_id,
                None,
            )?;
            let vm_space_handle = self.handles.insert(
                current.vm_space_id,
                ObjectKind::VmSpace,
                RIGHT_READ | RIGHT_WRITE | super::RIGHT_MAP | RIGHT_TRANSFER,
                current.process_id,
                None,
            )?;
            return Ok((process_handle, vm_space_handle));
        }
        if let Some(process) = self.processes.first() {
            let vm_space = self
                .vm_spaces
                .for_process(process.id)
                .ok_or(KernelServiceStatus::InvalidHandle)?;
            let process_handle = self.handles.insert(
                process.id,
                ObjectKind::Process,
                RIGHT_MANAGE_TASK | RIGHT_TRANSFER,
                process.id,
                None,
            )?;
            let vm_space_handle = self.handles.insert(
                vm_space.id,
                ObjectKind::VmSpace,
                RIGHT_READ | RIGHT_WRITE | super::RIGHT_MAP | RIGHT_TRANSFER,
                process.id,
                None,
            )?;
            return Ok((process_handle, vm_space_handle));
        }
        self.create_process(
            "kernel",
            RESOURCE_GROUP_SYSTEM,
            "bexos.kernel",
            super::HardwareAccess::Direct,
        )
    }
}

const fn wait_status(status: KernelServiceStatus) -> WaitManyResult {
    WaitManyResult {
        status,
        satisfied_index: u32::MAX,
        observed_signals: 0,
    }
}
use alloc::vec::Vec;
