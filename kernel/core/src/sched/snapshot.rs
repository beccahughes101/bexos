//! Owned scheduler state for kernel replacement; no native pointers cross it.
use super::*;
use crate::transplant::{
    TransplantError,
    codec::{Reader, Result, Writer},
};

const RUNTIME_MAGIC: u64 = u64::from_le_bytes(*b"BEXSCH02");
const MAGIC: u64 = u64::from_le_bytes(*b"BEXSCH01");
fn bad() -> TransplantError {
    TransplantError::InvalidRuntimeSnapshot
}
fn number<T: TryFrom<u64>>(r: &mut Reader<'_>) -> Result<T> {
    T::try_from(r.word()?).map_err(|_| bad())
}
fn write_optional(w: &mut Writer<'_>, value: Option<u64>) -> Result<()> {
    w.word(value.is_some() as u64)?;
    if let Some(value) = value {
        w.word(value)?;
    }
    Ok(())
}
fn read_optional(r: &mut Reader<'_>) -> Result<Option<u64>> {
    if r.flag()? {
        Ok(Some(r.word()?))
    } else {
        Ok(None)
    }
}

impl Scheduler {
    pub(crate) fn write_snapshot(&self, w: &mut Writer<'_>) -> Result<()> {
        for value in [
            if self.runtime.legacy {
                MAGIC
            } else {
                RUNTIME_MAGIC
            },
            self.total_ticks,
            self.now_ns,
            self.current_task_ids.len() as u64,
        ] {
            w.word(value)?;
        }
        for current in &self.current_task_ids {
            write_optional(w, *current)?;
        }
        w.word(self.tasks.len() as u64)?;
        for task in &self.tasks {
            write_task(w, task)?;
        }
        w.word(self.groups.len() as u64)?;
        for group in &self.groups {
            w.word(group.id as u64)?;
            write_optional(w, group.parent_id.map(u64::from))?;
            for value in [
                group.cpu_shares as u64,
                group.max_utilization_permille as u64,
                group.allow_realtime as u64,
                group.fair_vruntime,
                group.cpu_ticks,
                group.cpu_time_ns,
                group.quota_window_start_ns,
                group.quota_used_ns,
                group.runnable_tasks as u64,
            ] {
                w.word(value)?;
            }
        }
        w.word(self.fair_task_cursors.len() as u64)?;
        for (group, priority, cursor) in &self.fair_task_cursors {
            w.word(*group as u64)?;
            w.word(*priority as u64)?;
            write_optional(w, *cursor)?;
        }
        for word in self.fair_ready_bitmap {
            w.word(word)?;
        }
        if !self.runtime.legacy {
            write_optional(w, self.runtime.last_ns)?;
            w.word(self.runtime.threads.len() as u64)?;
            for &(id, process, cpu) in &self.runtime.threads {
                w.word(id)?;
                w.word(process)?;
                w.word(cpu)?;
            }
        }
        Ok(())
    }

    pub(crate) fn read_snapshot(r: &mut Reader<'_>) -> Result<Self> {
        let magic = r.word()?;
        if magic != MAGIC && magic != RUNTIME_MAGIC {
            return Err(bad());
        }
        let total_ticks = r.word()?;
        let now_ns = r.word()?;
        let count = r.count(MAX_CPUS as usize)?;
        if count == 0 {
            return Err(bad());
        }
        let mut value = Self::with_cpu_count(count as u32).map_err(|_| bad())?;
        value.total_ticks = total_ticks;
        value.now_ns = now_ns;
        for current in &mut value.current_task_ids {
            *current = read_optional(r)?;
        }
        for _ in 0..r.count(262144)? {
            let task = read_task(r)?;
            if value.tasks.iter().any(|other| other.id == task.id) {
                return Err(bad());
            }
            value.tasks.push(task);
        }
        for _ in 0..r.count(262144)? {
            let id = number(r)?;
            let parent_id = read_optional(r)?
                .map(u32::try_from)
                .transpose()
                .map_err(|_| bad())?;
            let group = ResourceGroupAccounting {
                id,
                parent_id,
                cpu_shares: number(r)?,
                max_utilization_permille: number(r)?,
                allow_realtime: r.flag()?,
                fair_vruntime: r.word()?,
                cpu_ticks: r.word()?,
                cpu_time_ns: r.word()?,
                quota_window_start_ns: r.word()?,
                quota_used_ns: r.word()?,
                runnable_tasks: number(r)?,
            };
            if id == 0
                || group.cpu_shares == 0
                || group.max_utilization_permille > 1000
                || value.groups.iter().any(|other| other.id == id)
            {
                return Err(bad());
            }
            value.groups.push(group);
        }
        for _ in 0..r.count(262144)? {
            value
                .fair_task_cursors
                .push((number(r)?, number(r)?, read_optional(r)?));
        }
        for word in &mut value.fair_ready_bitmap {
            *word = r.word()?;
        }
        value.runtime.legacy = magic == MAGIC;
        if magic == RUNTIME_MAGIC {
            value.runtime.last_ns = read_optional(r)?;
            for _ in 0..r.count(262144)? {
                let (id, process, cpu) = (r.word()?, r.word()?, r.word()?);
                if id == 0 || process == 0 {
                    return Err(bad());
                }
                value.runtime.register(id, process).map_err(|_| bad())?;
                value.runtime.threads.last_mut().unwrap().2 = cpu;
            }
            for task in &value.tasks {
                if !value
                    .runtime
                    .threads
                    .iter()
                    .any(|t| t.0 == task.id && t.1 == task.process_id)
                {
                    return Err(bad());
                }
            }
        } else {
            for task in &value.tasks {
                value
                    .runtime
                    .register(task.id, task.process_id)
                    .map_err(|_| bad())?;
            }
        }
        for (cpu, current) in value.current_task_ids.iter().enumerate() {
            if let Some(id) = current {
                let task = value
                    .tasks
                    .iter()
                    .find(|task| task.id == *id)
                    .ok_or_else(bad)?;
                if task.state != TaskState::Running || task.cpu_affinity_mask & (1 << cpu) == 0 {
                    return Err(bad());
                }
            }
        }
        for task in &value.tasks {
            if !value
                .groups
                .iter()
                .any(|group| group.id == task.resource_group_id)
                || task.cpu_affinity_mask & value.active_cpu_mask() == 0
            {
                return Err(bad());
            }
            if let Some(cpu) = task.assigned_cpu {
                // assigned_cpu is the last placement hint, including after
                // blocking/yielding. current_task_ids owns execution state.
                if cpu as usize >= value.current_task_ids.len() {
                    return Err(bad());
                }
            }
            if task.state == TaskState::Running && !value.current_task_ids.contains(&Some(task.id))
            {
                return Err(bad());
            }
        }
        // Preserve the scheduler's lazily refreshed runnable indexes exactly;
        // rebuilding them here changes valid state at a scheduling boundary.
        Ok(value)
    }
}

fn write_task(w: &mut Writer<'_>, task: &SchedulerTask) -> Result<()> {
    for value in [
        task.id,
        task.process_id,
        task.resource_group_id as u64,
        task.priority as u64,
        task.base_priority as u64,
        task.effective_priority as u64,
        task.fair_weight as u64,
    ] {
        w.word(value)?;
    }
    w.word(task.deadline.is_some() as u64)?;
    if let Some(deadline) = task.deadline {
        for value in [
            deadline.profile.capacity_ns,
            deadline.profile.deadline_ns,
            deadline.profile.period_ns,
            deadline.absolute_deadline_ns,
            deadline.remaining_budget_ns,
        ] {
            w.word(value)?;
        }
    }
    w.word(match task.state {
        TaskState::Ready => 0,
        TaskState::Running => 1,
        TaskState::Blocked => 2,
        TaskState::Suspended => 3,
        TaskState::Exited => 4,
        TaskState::Quiesced => 5,
        TaskState::Stopped => 6,
    })?;
    w.word(task.quantum_left as u64)?;
    w.word(task.quantum_deadline_ns)?;
    write_optional(w, task.assigned_cpu.map(u64::from))?;
    w.word(task.cpu_ticks)?;
    w.word(task.cpu_affinity_mask)?;
    write_block_reason(w, task.block_reason)?;
    w.word(task.exit_code as i64 as u64)?;
    w.word(task.name_len as u64)?;
    w.bytes(&task.name[..task.name_len])?;
    Ok(())
}

fn read_task(r: &mut Reader<'_>) -> Result<SchedulerTask> {
    let id = r.word()?;
    let process_id = r.word()?;
    let resource_group_id = number(r)?;
    let priority = number(r)?;
    let base_priority = number(r)?;
    let effective_priority = number(r)?;
    let fair_weight = number(r)?;
    let deadline = if r.flag()? {
        let profile = DeadlineProfile {
            capacity_ns: r.word()?,
            deadline_ns: r.word()?,
            period_ns: r.word()?,
        };
        let deadline = EffectiveDeadline {
            profile,
            absolute_deadline_ns: r.word()?,
            remaining_budget_ns: r.word()?,
        };
        if !profile.valid() || deadline.remaining_budget_ns > profile.capacity_ns {
            return Err(bad());
        }
        Some(deadline)
    } else {
        None
    };
    let state = match r.word()? {
        0 => TaskState::Ready,
        1 => TaskState::Running,
        2 => TaskState::Blocked,
        3 => TaskState::Suspended,
        4 => TaskState::Exited,
        5 => TaskState::Quiesced,
        6 => TaskState::Stopped,
        _ => return Err(bad()),
    };
    let quantum_left = number(r)?;
    let quantum_deadline_ns = r.word()?;
    let assigned_cpu = read_optional(r)?
        .map(u8::try_from)
        .transpose()
        .map_err(|_| bad())?;
    let cpu_ticks = r.word()?;
    let cpu_affinity_mask = r.word()?;
    let block_reason = read_block_reason(r)?;
    let exit_code = i32::try_from(r.word()? as i64).map_err(|_| bad())?;
    let name_len = r.count(MAX_TASK_NAME)?;
    let mut name = [0; MAX_TASK_NAME];
    name[..name_len].copy_from_slice(r.bytes(name_len)?);
    if id == 0 || process_id == 0 || resource_group_id == 0 || fair_weight == 0 || name_len == 0 {
        return Err(bad());
    }
    Ok(SchedulerTask {
        id,
        process_id,
        resource_group_id,
        priority,
        base_priority,
        effective_priority,
        fair_weight,
        deadline,
        state,
        quantum_left,
        quantum_deadline_ns,
        assigned_cpu,
        cpu_ticks,
        cpu_affinity_mask,
        block_reason,
        exit_code,
        name,
        name_len,
    })
}

fn write_block_reason(w: &mut Writer<'_>, reason: Option<BlockReason>) -> Result<()> {
    match reason {
        None => w.word(0)?,
        Some(BlockReason::Futex { uaddr }) => {
            w.word(1)?;
            w.word(uaddr)?;
        }
        Some(BlockReason::HandleSignals { handle, signals }) => {
            w.word(2)?;
            w.word(handle)?;
            w.word(signals as u64)?;
        }
        Some(BlockReason::WaitMany {
            items,
            item_count,
            deadline_nanos,
        }) => {
            w.word(3)?;
            w.word(item_count as u64)?;
            w.word(deadline_nanos)?;
            for (handle, signals) in items {
                w.word(handle)?;
                w.word(signals as u64)?;
            }
        }
        Some(BlockReason::SleepUntil { deadline_nanos }) => {
            w.word(4)?;
            w.word(deadline_nanos)?;
        }
        Some(BlockReason::FutexUntil {
            uaddr,
            deadline_nanos,
        }) => {
            w.word(5)?;
            w.word(uaddr)?;
            w.word(deadline_nanos)?;
        }
    }
    Ok(())
}
fn read_block_reason(r: &mut Reader<'_>) -> Result<Option<BlockReason>> {
    Ok(match r.word()? {
        0 => None,
        1 => Some(BlockReason::Futex { uaddr: r.word()? }),
        2 => Some(BlockReason::HandleSignals {
            handle: r.word()?,
            signals: number(r)?,
        }),
        3 => {
            let item_count = r.count(MAX_WAIT_MANY_ITEMS)? as u8;
            let deadline_nanos = r.word()?;
            let mut items = [(0, 0); MAX_WAIT_MANY_ITEMS];
            for item in &mut items {
                *item = (r.word()?, number(r)?);
            }
            Some(BlockReason::WaitMany {
                items,
                item_count,
                deadline_nanos,
            })
        }
        4 => Some(BlockReason::SleepUntil {
            deadline_nanos: r.word()?,
        }),
        5 => {
            let uaddr = r.word()?;
            if uaddr == 0 || uaddr & 3 != 0 {
                return Err(bad());
            }
            Some(BlockReason::FutexUntil {
                uaddr,
                deadline_nanos: r.word()?,
            })
        }
        _ => return Err(bad()),
    })
}
