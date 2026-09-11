pub const DEFAULT_QUANTUM_TICKS: u8 = 3;
mod quiescence;
mod snapshot;
/// The production scheduler accounts against the architectural monotonic clock.
/// `tick()` remains as a one-nanosecond model helper for older callers.
pub const DEFAULT_FAIR_QUANTUM_NS: u64 = 4_000_000;
/// Bound how far a runnable fair group can be charged ahead of its peers.
///
/// Userspace services currently wait cooperatively on driver replies. A long
/// driver operation must therefore retain bounded service after it completes a
/// CPU-heavy phase; otherwise polling clients can take seconds of virtual time
/// to catch up before the driver is scheduled again.
const MAX_FAIR_GROUP_LAG_QUANTA: u64 = 8;
pub const CPU_QUOTA_WINDOW_NS: u64 = 100_000_000;
pub const MAX_TASK_NAME: usize = 32;
pub const MAX_CPUS: u32 = 64;
pub const MAX_WAIT_MANY_ITEMS: usize = 8;
const FAIR_PRIORITY_WORDS: usize = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Task {
    pub id: u64,
    pub name: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerError {
    Full,
    InvalidTask,
    AlreadyExists,
    AccessDenied,
    ResourceExhausted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskState {
    Ready,
    Running,
    Blocked,
    Suspended,
    /// Held for ownership transfer. Ordinary wakeups may clear its wait but
    /// cannot make it runnable until the handover is aborted.
    Quiesced,
    /// Stopped by process control; ordinary I/O wakeups do not resume it.
    Stopped,
    Exited,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockReason {
    Futex {
        uaddr: u64,
    },
    FutexUntil {
        uaddr: u64,
        deadline_nanos: u64,
    },
    HandleSignals {
        handle: u64,
        signals: u32,
    },
    WaitMany {
        items: [(u64, u32); MAX_WAIT_MANY_ITEMS],
        item_count: u8,
        deadline_nanos: u64,
    },
    SleepUntil {
        deadline_nanos: u64,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FairProfile {
    pub priority: u8,
    pub weight: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeadlineProfile {
    pub capacity_ns: u64,
    pub deadline_ns: u64,
    pub period_ns: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulingProfile {
    Fair(FairProfile),
    Deadline(DeadlineProfile),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EffectiveDeadline {
    pub profile: DeadlineProfile,
    pub absolute_deadline_ns: u64,
    pub remaining_budget_ns: u64,
}

impl DeadlineProfile {
    pub const fn valid(self) -> bool {
        self.capacity_ns > 0
            && self.capacity_ns <= self.deadline_ns
            && self.deadline_ns <= self.period_ns
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchedulerTask {
    pub id: u64,
    pub process_id: u64,
    pub resource_group_id: u32,
    pub priority: u8,
    pub base_priority: u8,
    pub effective_priority: u8,
    pub fair_weight: u32,
    pub deadline: Option<EffectiveDeadline>,
    pub state: TaskState,
    pub quantum_left: u8,
    pub quantum_deadline_ns: u64,
    pub assigned_cpu: Option<u8>,
    pub cpu_ticks: u64,
    pub cpu_affinity_mask: u64,
    pub block_reason: Option<BlockReason>,
    pub exit_code: i32,
    pub name: [u8; MAX_TASK_NAME],
    pub name_len: usize,
}

impl SchedulerTask {
    pub fn new(
        id: u64,
        process_id: u64,
        resource_group_id: u32,
        priority: u8,
        name: &str,
    ) -> Result<Self, SchedulerError> {
        if id == 0 || process_id == 0 || name.is_empty() || name.len() > MAX_TASK_NAME {
            return Err(SchedulerError::InvalidTask);
        }
        let mut stored = [0; MAX_TASK_NAME];
        stored[..name.len()].copy_from_slice(name.as_bytes());
        Ok(Self {
            id,
            process_id,
            resource_group_id,
            priority,
            base_priority: priority,
            effective_priority: priority,
            fair_weight: 1,
            deadline: None,
            state: TaskState::Ready,
            quantum_left: DEFAULT_QUANTUM_TICKS,
            quantum_deadline_ns: 0,
            assigned_cpu: None,
            cpu_ticks: 0,
            cpu_affinity_mask: u64::MAX,
            block_reason: None,
            exit_code: 0,
            name: stored,
            name_len: name.len(),
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResourceGroupAccounting {
    pub id: u32,
    pub parent_id: Option<u32>,
    pub cpu_shares: u32,
    /// Zero means that this node adds no cap beyond its parent.
    pub max_utilization_permille: u16,
    pub allow_realtime: bool,
    pub fair_vruntime: u64,
    pub cpu_ticks: u64,
    pub cpu_time_ns: u64,
    pub quota_window_start_ns: u64,
    pub quota_used_ns: u64,
    pub runnable_tasks: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScheduleDecision {
    pub previous_task_id: Option<u64>,
    pub next_task_id: Option<u64>,
    pub cpu_id: u8,
    pub switched: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Scheduler {
    tasks: Vec<SchedulerTask>,
    runtime: runtime::Accounting,
    groups: Vec<ResourceGroupAccounting>,
    fair_task_cursors: Vec<(u32, u8, Option<u64>)>,
    current_task_ids: Vec<Option<u64>>,
    total_ticks: u64,
    now_ns: u64,
    fair_ready_bitmap: [u64; FAIR_PRIORITY_WORDS],
}

impl Scheduler {
    pub fn new() -> Self {
        Self::with_cpu_count(1).expect("single CPU scheduler")
    }

    pub fn with_cpu_count(max_cpus: u32) -> Result<Self, SchedulerError> {
        if max_cpus == 0 || max_cpus > MAX_CPUS {
            return Err(SchedulerError::InvalidTask);
        }
        let mut current_task_ids = Vec::new();
        current_task_ids
            .try_reserve(max_cpus as usize)
            .map_err(|_| SchedulerError::Full)?;
        current_task_ids.resize(max_cpus as usize, None);
        Ok(Self {
            tasks: Vec::new(),
            runtime: runtime::Accounting::new(),
            groups: Vec::new(),
            fair_task_cursors: Vec::new(),
            current_task_ids,
            total_ticks: 0,
            now_ns: 0,
            fair_ready_bitmap: [0; FAIR_PRIORITY_WORDS],
        })
    }

    pub const fn empty() -> Self {
        Self {
            tasks: Vec::new(),
            runtime: runtime::Accounting::new(),
            groups: Vec::new(),
            fair_task_cursors: Vec::new(),
            current_task_ids: Vec::new(),
            total_ticks: 0,
            now_ns: 0,
            fair_ready_bitmap: [0; FAIR_PRIORITY_WORDS],
        }
    }

    pub fn ensure_resource_group(&mut self, id: u32) -> Result<(), SchedulerError> {
        if self.groups.iter().any(|group| group.id == id) {
            return Ok(());
        }
        self.ensure_resource_group_with_limits(id, None, 1024, 0, true)
    }

    pub fn ensure_resource_group_with_shares(
        &mut self,
        id: u32,
        cpu_shares: u32,
    ) -> Result<(), SchedulerError> {
        self.ensure_resource_group_with_limits(id, None, cpu_shares, 0, true)
    }

    pub fn ensure_resource_group_with_limits(
        &mut self,
        id: u32,
        parent_id: Option<u32>,
        cpu_shares: u32,
        max_utilization_permille: u16,
        allow_realtime: bool,
    ) -> Result<(), SchedulerError> {
        if id == 0 || cpu_shares == 0 || max_utilization_permille > 1000 {
            return Err(SchedulerError::InvalidTask);
        }
        if parent_id == Some(id)
            || parent_id.is_some_and(|parent| !self.groups.iter().any(|group| group.id == parent))
        {
            return Err(SchedulerError::InvalidTask);
        }
        if let Some(group) = self.groups.iter_mut().find(|group| group.id == id) {
            if group.parent_id != parent_id && group.parent_id.is_some() {
                return Err(SchedulerError::AccessDenied);
            }
            group.parent_id = parent_id;
            group.cpu_shares = cpu_shares;
            group.max_utilization_permille = max_utilization_permille;
            group.allow_realtime = allow_realtime;
            return Ok(());
        }
        self.groups
            .try_reserve(1)
            .map_err(|_| SchedulerError::Full)?;
        self.fair_task_cursors
            .try_reserve(self.cpu_count() as usize)
            .map_err(|_| SchedulerError::Full)?;
        let fair_vruntime = self
            .groups
            .iter()
            .filter(|group| group.runnable_tasks != 0)
            .map(|group| group.fair_vruntime)
            .min()
            .unwrap_or(0);
        self.groups.push(ResourceGroupAccounting {
            id,
            parent_id,
            cpu_shares,
            max_utilization_permille,
            allow_realtime,
            fair_vruntime,
            cpu_ticks: 0,
            cpu_time_ns: 0,
            quota_window_start_ns: self.now_ns,
            quota_used_ns: 0,
            runnable_tasks: 0,
        });
        for cpu_id in 0..self.cpu_count() {
            self.fair_task_cursors.push((id, cpu_id, None));
        }
        Ok(())
    }

    pub fn set_resource_group_shares(
        &mut self,
        id: u32,
        cpu_shares: u32,
    ) -> Result<(), SchedulerError> {
        if cpu_shares == 0 {
            return Err(SchedulerError::InvalidTask);
        }
        let Some(group) = self.groups.iter_mut().find(|group| group.id == id) else {
            return Err(SchedulerError::InvalidTask);
        };
        group.cpu_shares = cpu_shares;
        Ok(())
    }

    pub fn set_resource_group_limits(
        &mut self,
        id: u32,
        cpu_shares: u32,
        max_utilization_permille: u16,
        allow_realtime: bool,
    ) -> Result<(), SchedulerError> {
        if cpu_shares == 0 || max_utilization_permille > 1000 {
            return Err(SchedulerError::InvalidTask);
        }
        let Some(group) = self.groups.iter_mut().find(|group| group.id == id) else {
            return Err(SchedulerError::InvalidTask);
        };
        group.cpu_shares = cpu_shares;
        group.max_utilization_permille = max_utilization_permille;
        group.allow_realtime = allow_realtime;
        Ok(())
    }

    pub fn resource_group(&self, id: u32) -> Option<ResourceGroupAccounting> {
        self.groups.iter().find(|group| group.id == id).copied()
    }

    pub const fn now_ns(&self) -> u64 {
        self.now_ns
    }

    /// Returns the earliest local scheduling event, if the CPU has work or a
    /// blocked timeout. Hardware code programs its one-shot timer from this.
    pub fn next_deadline_on_cpu(&self, cpu_id: u8) -> Result<Option<u64>, SchedulerError> {
        self.ensure_cpu_id(cpu_id)?;
        let running_deadline = self.current_on_cpu(cpu_id).and_then(|task| {
            task.deadline
                .map(|deadline| self.now_ns.saturating_add(deadline.remaining_budget_ns))
                .or_else(|| (task.quantum_deadline_ns != 0).then_some(task.quantum_deadline_ns))
        });
        let wake_deadline = self
            .tasks
            .iter()
            .filter_map(|task| match task.block_reason {
                Some(BlockReason::SleepUntil { deadline_nanos }) => Some(deadline_nanos),
                Some(BlockReason::WaitMany { deadline_nanos, .. }) => Some(deadline_nanos),
                Some(BlockReason::FutexUntil { deadline_nanos, .. }) => Some(deadline_nanos),
                _ => None,
            })
            .min();
        let replenish_deadline = self
            .tasks
            .iter()
            .filter(|task| {
                task.state == TaskState::Ready && self.task_can_run_on_cpu(**task, cpu_id)
            })
            .filter_map(|task| task.deadline.filter(|d| d.remaining_budget_ns == 0))
            .map(|d| {
                d.absolute_deadline_ns
                    .saturating_sub(d.profile.deadline_ns)
                    .saturating_add(d.profile.period_ns)
            })
            .min();
        let wake_deadline = wake_deadline.into_iter().chain(replenish_deadline).min();
        Ok(match (running_deadline, wake_deadline) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) | (None, Some(a)) => Some(a),
            (None, None) => None,
        })
    }

    pub fn add_task(&mut self, mut task: SchedulerTask) -> Result<(), SchedulerError> {
        if self.tasks.iter().any(|existing| existing.id == task.id) {
            return Err(SchedulerError::AlreadyExists);
        }
        if self.current_task_ids.is_empty() {
            self.current_task_ids.push(None);
        }
        task.cpu_affinity_mask &= self.active_cpu_mask();
        if task.cpu_affinity_mask == 0 {
            return Err(SchedulerError::InvalidTask);
        }
        self.ensure_resource_group(task.resource_group_id)?;
        let fair_baseline = self
            .groups
            .iter()
            .filter(|group| group.runnable_tasks != 0)
            .map(|group| group.fair_vruntime)
            .min();
        if let Some(baseline) = fair_baseline {
            if let Some(group) = self
                .groups
                .iter_mut()
                .find(|group| group.id == task.resource_group_id && group.runnable_tasks == 0)
            {
                group.fair_vruntime = baseline;
            }
        }
        self.tasks
            .try_reserve(1)
            .map_err(|_| SchedulerError::Full)?;
        self.runtime.register(task.id, task.process_id)?;
        self.tasks.push(task);
        self.rebuild_runnable_indexes();
        for cpu_id in 0..self.cpu_count() {
            if self.current_task_ids[cpu_id as usize].is_none() {
                let _ = self.schedule_next_on_cpu(cpu_id);
            }
        }
        Ok(())
    }

    pub fn current(&self) -> Option<SchedulerTask> {
        self.current_on_cpu(0)
    }

    pub fn current_on_cpu(&self, cpu_id: u8) -> Option<SchedulerTask> {
        let id = self
            .current_task_ids
            .get(cpu_id as usize)
            .copied()
            .flatten()?;
        self.task(id)
    }

    pub fn task(&self, id: u64) -> Option<SchedulerTask> {
        self.tasks.iter().find(|task| task.id == id).copied()
    }

    /// Lightweight state traversal for runtime wake reconciliation. Avoids
    /// repeated ID searches and copying each task's bounded WaitMany array.
    pub fn block_states(&self) -> impl Iterator<Item = (u64, bool)> + '_ {
        self.tasks
            .iter()
            .map(|task| (task.id, task.block_reason.is_some()))
    }

    pub fn tick(&mut self) -> ScheduleDecision {
        self.tick_at_on_cpu(0, self.now_ns.saturating_add(1))
            .unwrap_or(ScheduleDecision {
                previous_task_id: None,
                next_task_id: None,
                cpu_id: 0,
                switched: false,
            })
    }

    pub fn tick_on_cpu(&mut self, cpu_id: u8) -> Result<ScheduleDecision, SchedulerError> {
        self.tick_at_on_cpu(cpu_id, self.now_ns.saturating_add(1))
    }

    pub fn tick_at_on_cpu(
        &mut self,
        cpu_id: u8,
        now_ns: u64,
    ) -> Result<ScheduleDecision, SchedulerError> {
        self.ensure_cpu_id(cpu_id)?;
        if now_ns < self.now_ns {
            return Err(SchedulerError::InvalidTask);
        }
        self.account_runtime(now_ns);
        let elapsed_ns = now_ns.saturating_sub(self.now_ns);
        self.now_ns = now_ns;
        self.total_ticks = self.total_ticks.saturating_add(1);
        self.wake_expired_deadlines();
        let previous = self.current_task_ids[cpu_id as usize];
        if let Some(current_id) = previous {
            let mut tick_group = None;
            let mut continue_running = false;
            if let Some(current) = self.task_mut(current_id) {
                current.cpu_ticks = current.cpu_ticks.saturating_add(1);
                tick_group = Some((current.resource_group_id, current.deadline.is_none()));
                if let Some(deadline) = current.deadline.as_mut() {
                    // account_runtime charged every running CPU once, including
                    // execution before intervening syscalls and voluntary yields.
                    if deadline.remaining_budget_ns > 0 && current.state == TaskState::Running {
                        continue_running = true;
                    }
                } else {
                    current.quantum_left = current.quantum_left.saturating_sub(1);
                    if current.quantum_deadline_ns == 0 {
                        current.quantum_deadline_ns =
                            now_ns.saturating_add(DEFAULT_FAIR_QUANTUM_NS);
                    }
                    if current.quantum_left > 0
                        && current.quantum_deadline_ns > now_ns
                        && current.state == TaskState::Running
                    {
                        continue_running = true;
                    }
                }
                if continue_running {
                    self.add_group_tick_if_present(tick_group, elapsed_ns);
                    return Ok(ScheduleDecision {
                        previous_task_id: previous,
                        next_task_id: previous,
                        cpu_id,
                        switched: false,
                    });
                }
                if current.state == TaskState::Running {
                    current.state = TaskState::Ready;
                    current.quantum_left = DEFAULT_QUANTUM_TICKS;
                }
            }
            self.add_group_tick_if_present(tick_group, elapsed_ns);
        }
        let next = self.schedule_next_on_cpu(cpu_id);
        Ok(ScheduleDecision {
            previous_task_id: previous,
            next_task_id: next,
            cpu_id,
            switched: previous != next,
        })
    }

    pub fn block_current(
        &mut self,
        reason: BlockReason,
    ) -> Result<ScheduleDecision, SchedulerError> {
        self.block_current_on_cpu(0, reason)
    }

    pub fn block_current_on_cpu(
        &mut self,
        cpu_id: u8,
        reason: BlockReason,
    ) -> Result<ScheduleDecision, SchedulerError> {
        self.ensure_cpu_id(cpu_id)?;
        let Some(current_id) = self.current_task_ids[cpu_id as usize] else {
            return Err(SchedulerError::InvalidTask);
        };
        let Some(current) = self.task_mut(current_id) else {
            return Err(SchedulerError::InvalidTask);
        };
        current.state = TaskState::Blocked;
        current.block_reason = Some(reason);
        current.quantum_left = DEFAULT_QUANTUM_TICKS;
        self.rebuild_runnable_indexes();
        self.current_task_ids[cpu_id as usize] = None;
        let next = self.schedule_next_on_cpu(cpu_id);
        Ok(ScheduleDecision {
            previous_task_id: Some(current_id),
            next_task_id: next,
            cpu_id,
            switched: Some(current_id) != next,
        })
    }

    pub fn wake_task(&mut self, id: u64) -> Result<(), SchedulerError> {
        let Some(task) = self.task_mut(id) else {
            return Err(SchedulerError::InvalidTask);
        };
        if task.state == TaskState::Blocked || task.state == TaskState::Suspended {
            task.state = TaskState::Ready;
            task.block_reason = None;
            task.quantum_left = DEFAULT_QUANTUM_TICKS;
        } else if matches!(task.state, TaskState::Quiesced | TaskState::Stopped) {
            task.block_reason = None;
        }
        self.rebuild_runnable_indexes();
        for cpu_id in 0..self.cpu_count() {
            if self.current_task_ids[cpu_id as usize].is_none() {
                let _ = self.schedule_next_on_cpu(cpu_id);
            }
        }
        Ok(())
    }

    pub fn wake_handle_waiters(&mut self, handle: u64, signals: u32) -> usize {
        let mut woken = 0;
        let ids = self
            .tasks
            .iter()
            .filter_map(|task| match task.block_reason {
                Some(BlockReason::HandleSignals {
                    handle: waited,
                    signals: wanted,
                }) if waited == handle && wanted & signals != 0 => Some(task.id),
                Some(BlockReason::WaitMany {
                    items, item_count, ..
                }) if wait_many_matches(items, item_count, handle, signals) => Some(task.id),
                _ => None,
            })
            .collect::<Vec<_>>();
        for id in ids {
            if self.wake_task(id).is_ok() {
                woken += 1;
            }
        }
        woken
    }

    pub fn wake_expired_deadlines(&mut self) -> usize {
        let now = self.now_ns;
        let ids = self
            .tasks
            .iter()
            .filter_map(|task| match task.block_reason {
                Some(BlockReason::SleepUntil { deadline_nanos }) if deadline_nanos <= now => {
                    Some(task.id)
                }
                Some(BlockReason::WaitMany { deadline_nanos, .. }) if deadline_nanos <= now => {
                    Some(task.id)
                }
                Some(BlockReason::FutexUntil { deadline_nanos, .. }) if deadline_nanos <= now => {
                    Some(task.id)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let mut woken = 0;
        for id in ids {
            if self.wake_task(id).is_ok() {
                woken += 1;
            }
        }
        woken
    }

    pub fn suspend_task(&mut self, id: u64) -> Result<(), SchedulerError> {
        let Some(task) = self.task_mut(id) else {
            return Err(SchedulerError::InvalidTask);
        };
        if task.state != TaskState::Exited {
            task.state = TaskState::Suspended;
            task.quantum_left = DEFAULT_QUANTUM_TICKS;
        }
        for cpu_id in 0..self.cpu_count() {
            if self.current_task_ids[cpu_id as usize] == Some(id) {
                self.current_task_ids[cpu_id as usize] = None;
                let _ = self.schedule_next_on_cpu(cpu_id);
            }
        }
        self.rebuild_runnable_indexes();
        Ok(())
    }

    pub fn exit_current(&mut self, exit_code: i32) -> Result<ScheduleDecision, SchedulerError> {
        self.exit_current_on_cpu(0, exit_code)
    }

    pub fn exit_task(&mut self, id: u64, exit_code: i32) -> Result<(), SchedulerError> {
        let Some(task) = self.task_mut(id) else {
            return Err(SchedulerError::InvalidTask);
        };
        task.state = TaskState::Exited;
        task.exit_code = exit_code;
        task.quantum_left = 0;
        task.block_reason = None;
        for cpu_id in 0..self.cpu_count() {
            if self.current_task_ids[cpu_id as usize] == Some(id) {
                self.current_task_ids[cpu_id as usize] = None;
            }
        }
        self.rebuild_runnable_indexes();
        for cpu_id in 0..self.cpu_count() {
            if self.current_task_ids[cpu_id as usize].is_none() {
                let _ = self.schedule_next_on_cpu(cpu_id);
            }
        }
        Ok(())
    }

    pub fn exit_current_on_cpu(
        &mut self,
        cpu_id: u8,
        exit_code: i32,
    ) -> Result<ScheduleDecision, SchedulerError> {
        self.ensure_cpu_id(cpu_id)?;
        let Some(current_id) = self.current_task_ids[cpu_id as usize] else {
            return Err(SchedulerError::InvalidTask);
        };
        let Some(task) = self.task_mut(current_id) else {
            return Err(SchedulerError::InvalidTask);
        };
        task.state = TaskState::Exited;
        task.exit_code = exit_code;
        task.quantum_left = 0;
        self.rebuild_runnable_indexes();
        self.current_task_ids[cpu_id as usize] = None;
        let next = self.schedule_next_on_cpu(cpu_id);
        Ok(ScheduleDecision {
            previous_task_id: Some(current_id),
            next_task_id: next,
            cpu_id,
            switched: Some(current_id) != next,
        })
    }

    pub fn cleanup_exited(&mut self) -> usize {
        let mut removed = 0;
        self.tasks.retain(|task| {
            let keep = task.state != TaskState::Exited;
            if !keep {
                removed += 1;
            }
            keep
        });
        let active_ids = self.tasks.iter().map(|task| task.id).collect::<Vec<_>>();
        for current in &mut self.current_task_ids {
            if current.is_some_and(|id| !active_ids.contains(&id)) {
                *current = None;
            }
        }
        self.rebuild_runnable_indexes();
        removed
    }

    pub fn set_profile(
        &mut self,
        id: u64,
        profile: SchedulingProfile,
    ) -> Result<(), SchedulerError> {
        match profile {
            SchedulingProfile::Fair(profile) => self.set_fair_profile(id, profile),
            SchedulingProfile::Deadline(profile) => self.set_deadline_profile(id, profile),
        }
    }

    pub fn set_fair_profile(
        &mut self,
        id: u64,
        profile: FairProfile,
    ) -> Result<(), SchedulerError> {
        if profile.priority == 0 || profile.weight == 0 {
            return Err(SchedulerError::InvalidTask);
        }
        let Some(task) = self.task_mut(id) else {
            return Err(SchedulerError::InvalidTask);
        };
        task.priority = profile.priority;
        task.base_priority = profile.priority;
        task.effective_priority = profile.priority;
        task.fair_weight = profile.weight;
        task.deadline = None;
        task.quantum_left = DEFAULT_QUANTUM_TICKS;
        self.rebuild_runnable_indexes();
        Ok(())
    }

    pub fn set_deadline_profile(
        &mut self,
        id: u64,
        profile: DeadlineProfile,
    ) -> Result<(), SchedulerError> {
        if !profile.valid() {
            return Err(SchedulerError::InvalidTask);
        }
        let group_id = self
            .task(id)
            .ok_or(SchedulerError::InvalidTask)?
            .resource_group_id;
        if !self.group_allows_realtime(group_id) {
            return Err(SchedulerError::AccessDenied);
        }
        let utilization_ppm = self
            .tasks
            .iter()
            .filter(|task| task.id != id && task.state != TaskState::Exited)
            .filter_map(|task| task.deadline.map(|deadline| deadline.profile))
            .fold(0u128, |sum, deadline| {
                sum.saturating_add(utilization_ppm(deadline))
            })
            .saturating_add(utilization_ppm(profile));
        if utilization_ppm > 1_000_000 {
            return Err(SchedulerError::ResourceExhausted);
        }
        let now = self.now_ns;
        let Some(task) = self.task_mut(id) else {
            return Err(SchedulerError::InvalidTask);
        };
        task.deadline = Some(EffectiveDeadline {
            profile,
            absolute_deadline_ns: now.saturating_add(profile.deadline_ns),
            remaining_budget_ns: profile.capacity_ns,
        });
        task.effective_priority = u8::MAX;
        task.priority = u8::MAX;
        task.quantum_left = DEFAULT_QUANTUM_TICKS;
        self.rebuild_runnable_indexes();
        Ok(())
    }

    pub fn donate_priority(
        &mut self,
        owner_id: u64,
        donated_priority: u8,
    ) -> Result<(), SchedulerError> {
        let Some(task) = self.task_mut(owner_id) else {
            return Err(SchedulerError::InvalidTask);
        };
        if task.deadline.is_none() && donated_priority > task.effective_priority {
            task.effective_priority = donated_priority;
            task.priority = donated_priority;
        }
        self.rebuild_runnable_indexes();
        Ok(())
    }

    pub fn restore_base_priority(&mut self, id: u64) -> Result<(), SchedulerError> {
        let Some(task) = self.task_mut(id) else {
            return Err(SchedulerError::InvalidTask);
        };
        if task.deadline.is_none() {
            task.effective_priority = task.base_priority;
            task.priority = task.base_priority;
        }
        self.rebuild_runnable_indexes();
        Ok(())
    }

    pub fn yield_current_to(
        &mut self,
        target_id: Option<u64>,
    ) -> Result<ScheduleDecision, SchedulerError> {
        self.yield_current_to_on_cpu(0, target_id)
    }

    pub fn yield_current_to_on_cpu(
        &mut self,
        cpu_id: u8,
        target_id: Option<u64>,
    ) -> Result<ScheduleDecision, SchedulerError> {
        self.ensure_cpu_id(cpu_id)?;
        let previous = self.current_task_ids[cpu_id as usize];
        if let Some(id) = target_id {
            let Some(target) = self.task(id) else {
                return Err(SchedulerError::InvalidTask);
            };
            if target.state != TaskState::Ready
                || !self.task_can_run_on_cpu(target, cpu_id)
                || self.is_running_elsewhere(id, cpu_id)
            {
                return Err(SchedulerError::InvalidTask);
            }
        }
        let mut yielded_current = None;
        let mut yielded_group = None;
        if let Some(current_id) = previous {
            if let Some(current) = self.task_mut(current_id) {
                if current.state == TaskState::Running {
                    yielded_group = Some((current.resource_group_id, current.deadline.is_none()));
                    current.state = TaskState::Ready;
                    yielded_current = Some(current_id);
                }
            }
        }
        // A voluntary yield consumes at least one fair scheduling quantum.
        // Charging a single nanosecond lets polling groups run millions of
        // turns after a peer is charged for one timer quantum.
        self.add_group_tick_if_present(yielded_group, DEFAULT_FAIR_QUANTUM_NS);
        let next = match target_id {
            Some(id) => {
                self.current_task_ids[cpu_id as usize] = Some(id);
                if let Some(target) = self.task_mut(id) {
                    target.state = TaskState::Running;
                    target.quantum_left = DEFAULT_QUANTUM_TICKS;
                }
                Some(id)
            }
            None => {
                if let Some(current_id) = yielded_current {
                    if let Some(current) = self.task_mut(current_id) {
                        current.state = TaskState::Suspended;
                    }
                }
                let mut next = self.schedule_next_on_cpu(cpu_id);
                if let Some(current_id) = yielded_current {
                    if let Some(current) = self.task_mut(current_id) {
                        current.state = TaskState::Ready;
                    }
                }
                if next.is_none() {
                    next = self.schedule_next_on_cpu(cpu_id);
                }
                next
            }
        };
        self.rebuild_runnable_indexes();
        Ok(ScheduleDecision {
            previous_task_id: previous,
            next_task_id: next,
            cpu_id,
            switched: previous != next,
        })
    }

    pub fn yield_current_to_at_on_cpu(
        &mut self,
        cpu_id: u8,
        target_id: Option<u64>,
        now_ns: u64,
    ) -> Result<ScheduleDecision, SchedulerError> {
        self.ensure_cpu_id(cpu_id)?;
        if now_ns < self.now_ns {
            return Err(SchedulerError::InvalidTask);
        }
        self.account_runtime(now_ns);
        // A voluntary yield is a scheduling boundary, so refresh the clock
        // before selecting the next task and programming its quantum. Runtime
        // accounting above spends deadline budgets; fair groups retain their
        // bounded synthetic yield charge.
        self.now_ns = now_ns;
        self.wake_expired_deadlines();
        self.yield_current_to_on_cpu(cpu_id, target_id)
    }

    pub fn set_cpu_affinity(&mut self, id: u64, affinity_mask: u64) -> Result<(), SchedulerError> {
        if affinity_mask == 0 || affinity_mask & !self.active_cpu_mask() != 0 {
            return Err(SchedulerError::InvalidTask);
        }
        let Some(task) = self.task_mut(id) else {
            return Err(SchedulerError::InvalidTask);
        };
        task.cpu_affinity_mask = affinity_mask;
        for cpu_id in 0..self.cpu_count() {
            if self.current_task_ids[cpu_id as usize] == Some(id)
                && affinity_mask & (1u64 << cpu_id) == 0
            {
                if let Some(task) = self.task_mut(id) {
                    task.state = TaskState::Ready;
                }
                self.current_task_ids[cpu_id as usize] = None;
                let _ = self.schedule_next_on_cpu(cpu_id);
            }
        }
        self.rebuild_runnable_indexes();
        Ok(())
    }

    pub fn group_accounting(&self, id: u32) -> Option<ResourceGroupAccounting> {
        self.groups.iter().find(|group| group.id == id).copied()
    }

    pub const fn ticks(&self) -> u64 {
        self.total_ticks
    }

    pub fn cpu_count(&self) -> u8 {
        self.current_task_ids.len().max(1) as u8
    }

    pub fn active_cpu_mask(&self) -> u64 {
        cpu_mask(self.cpu_count())
    }

    pub fn len(&self) -> usize {
        self.tasks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn schedule_next_on_cpu(&mut self, cpu_id: u8) -> Option<u64> {
        // Exhausted servers remain ready but ineligible until their next period.
        // Refilling immediately lets a continuously rendering client starve fair
        // storage and display services despite passing utilization admission.
        for task in &mut self.tasks {
            if task.state == TaskState::Running {
                continue;
            }
            if let Some(deadline) = task.deadline.as_mut() {
                let start = deadline
                    .absolute_deadline_ns
                    .saturating_sub(deadline.profile.deadline_ns);
                let periods = self.now_ns.saturating_sub(start) / deadline.profile.period_ns;
                if periods > 0 {
                    deadline.absolute_deadline_ns = deadline
                        .absolute_deadline_ns
                        .saturating_add(periods.saturating_mul(deadline.profile.period_ns));
                    deadline.remaining_budget_ns = deadline.profile.capacity_ns;
                }
            }
        }
        let previous = self.current_task_ids[cpu_id as usize];
        let next = self
            .next_deadline_ready(cpu_id)
            .or_else(|| self.next_fair_ready(cpu_id, previous));
        self.current_task_ids[cpu_id as usize] = next;
        if let Some(next_id) = next {
            let quantum_deadline_ns = self.now_ns.saturating_add(DEFAULT_FAIR_QUANTUM_NS);
            if let Some(task) = self.task_mut(next_id) {
                task.state = TaskState::Running;
                task.quantum_left = DEFAULT_QUANTUM_TICKS;
                task.quantum_deadline_ns = quantum_deadline_ns;
                task.assigned_cpu = Some(cpu_id);
            }
        }
        next
    }

    fn next_deadline_ready(&self, cpu_id: u8) -> Option<u64> {
        self.tasks
            .iter()
            .filter(|task| task.state == TaskState::Ready)
            .filter(|task| self.task_can_run_on_cpu(**task, cpu_id))
            .filter(|task| !self.is_running_elsewhere(task.id, cpu_id))
            .filter_map(|task| {
                task.deadline
                    .filter(|deadline| deadline.remaining_budget_ns > 0)
                    .map(|deadline| (task.id, deadline.absolute_deadline_ns))
            })
            .min_by_key(|(_, deadline)| *deadline)
            .map(|(id, _)| id)
    }

    fn next_fair_ready(&mut self, cpu_id: u8, _previous: Option<u64>) -> Option<u64> {
        let group_id = self.next_fair_group(cpu_id)?;
        let priority = highest_ready_priority(self.fair_ready_bitmap_for_group(group_id, cpu_id))?;
        let previous = self
            .fair_task_cursors
            .iter()
            .find(|(cursor_group, cursor_cpu, _)| {
                *cursor_group == group_id && *cursor_cpu == cpu_id
            })
            .and_then(|(_, _, task_id)| *task_id);
        let start_index = previous
            .and_then(|id| self.tasks.iter().position(|task| task.id == id))
            .map(|index| {
                if self.tasks.is_empty() {
                    0
                } else {
                    (index + 1) % self.tasks.len()
                }
            })
            .unwrap_or(0);

        for offset in 0..self.tasks.len() {
            let index = (start_index + offset) % self.tasks.len();
            let task = self.tasks[index];
            if task.state == TaskState::Ready
                && task.deadline.is_none()
                && task.resource_group_id == group_id
                && task.effective_priority == priority
                && self.task_can_run_on_cpu(task, cpu_id)
                && !self.is_running_elsewhere(task.id, cpu_id)
            {
                if let Some((_, _, cursor)) =
                    self.fair_task_cursors
                        .iter_mut()
                        .find(|(cursor_group, cursor_cpu, _)| {
                            *cursor_group == group_id && *cursor_cpu == cpu_id
                        })
                {
                    *cursor = Some(task.id);
                }
                return Some(task.id);
            }
        }
        None
    }

    fn next_fair_group(&self, cpu_id: u8) -> Option<u32> {
        self.groups
            .iter()
            .filter(|group| self.group_has_quota(group.id))
            .filter(|group| {
                self.tasks.iter().any(|task| {
                    task.resource_group_id == group.id
                        && task.state == TaskState::Ready
                        && task.deadline.is_none()
                        && self.task_can_run_on_cpu(*task, cpu_id)
                        && !self.is_running_elsewhere(task.id, cpu_id)
                })
            })
            .min_by_key(|group| group.fair_vruntime)
            .map(|group| group.id)
    }

    fn fair_ready_bitmap_for_group(&self, group_id: u32, cpu_id: u8) -> [u64; FAIR_PRIORITY_WORDS] {
        let mut bitmap = [0; FAIR_PRIORITY_WORDS];
        for task in &self.tasks {
            if task.resource_group_id == group_id
                && task.state == TaskState::Ready
                && task.deadline.is_none()
                && self.task_can_run_on_cpu(*task, cpu_id)
                && !self.is_running_elsewhere(task.id, cpu_id)
            {
                set_ready_priority(&mut bitmap, task.effective_priority);
            }
        }
        bitmap
    }

    fn task_mut(&mut self, id: u64) -> Option<&mut SchedulerTask> {
        self.tasks.iter_mut().find(|task| task.id == id)
    }

    fn add_group_tick(&mut self, id: u32, charge_fair_runtime: bool, elapsed_ns: u64) {
        let mut ancestor = Some(id);
        while let Some(group_id) = ancestor {
            let Some(index) = self.groups.iter().position(|group| group.id == group_id) else {
                break;
            };
            let group = &mut self.groups[index];
            if self.now_ns.saturating_sub(group.quota_window_start_ns) >= CPU_QUOTA_WINDOW_NS {
                group.quota_window_start_ns = self.now_ns;
                group.quota_used_ns = 0;
            }
            group.cpu_ticks = group.cpu_ticks.saturating_add(1);
            group.cpu_time_ns = group.cpu_time_ns.saturating_add(elapsed_ns);
            group.quota_used_ns = group.quota_used_ns.saturating_add(elapsed_ns);
            if charge_fair_runtime {
                group.fair_vruntime = group.fair_vruntime.saturating_add(
                    elapsed_ns.saturating_mul(1024).saturating_mul(1024)
                        / u64::from(group.cpu_shares),
                );
            }
            ancestor = group.parent_id;
        }
    }

    fn add_group_tick_if_present(&mut self, tick_group: Option<(u32, bool)>, elapsed_ns: u64) {
        if let Some((id, charge_fair_runtime)) = tick_group {
            self.add_group_tick(id, charge_fair_runtime, elapsed_ns);
            if charge_fair_runtime {
                self.clamp_runnable_group_lag();
            }
        }
    }

    fn clamp_runnable_group_lag(&mut self) {
        let Some(min_vruntime) = self
            .groups
            .iter()
            .filter(|group| group.runnable_tasks != 0)
            .map(|group| group.fair_vruntime)
            .min()
        else {
            return;
        };
        let max_vruntime = min_vruntime.saturating_add(
            DEFAULT_FAIR_QUANTUM_NS
                .saturating_mul(1024)
                .saturating_mul(MAX_FAIR_GROUP_LAG_QUANTA),
        );
        for group in self
            .groups
            .iter_mut()
            .filter(|group| group.runnable_tasks != 0)
        {
            group.fair_vruntime = group.fair_vruntime.min(max_vruntime);
        }
    }

    fn group_has_quota(&self, id: u32) -> bool {
        let mut ancestor = Some(id);
        while let Some(group_id) = ancestor {
            let Some(group) = self.groups.iter().find(|group| group.id == group_id) else {
                return false;
            };
            if group.max_utilization_permille != 0 {
                let budget = CPU_QUOTA_WINDOW_NS
                    .saturating_mul(u64::from(group.max_utilization_permille))
                    / 1000;
                if group.quota_used_ns >= budget {
                    return false;
                }
            }
            ancestor = group.parent_id;
        }
        true
    }

    fn group_allows_realtime(&self, id: u32) -> bool {
        let mut ancestor = Some(id);
        while let Some(group_id) = ancestor {
            let Some(group) = self.groups.iter().find(|group| group.id == group_id) else {
                return false;
            };
            if !group.allow_realtime {
                return false;
            }
            ancestor = group.parent_id;
        }
        true
    }

    fn ensure_cpu_id(&self, cpu_id: u8) -> Result<(), SchedulerError> {
        if (cpu_id as usize) < self.current_task_ids.len() {
            Ok(())
        } else {
            Err(SchedulerError::InvalidTask)
        }
    }

    fn task_can_run_on_cpu(&self, task: SchedulerTask, cpu_id: u8) -> bool {
        task.cpu_affinity_mask & (1u64 << cpu_id) != 0
    }

    fn is_running_elsewhere(&self, task_id: u64, cpu_id: u8) -> bool {
        self.current_task_ids
            .iter()
            .enumerate()
            .any(|(index, current)| index != cpu_id as usize && *current == Some(task_id))
    }

    fn rebuild_runnable_indexes(&mut self) {
        self.fair_ready_bitmap = [0; FAIR_PRIORITY_WORDS];
        for group in &mut self.groups {
            group.runnable_tasks = 0;
        }
        for task in &self.tasks {
            if task.state == TaskState::Ready || task.state == TaskState::Running {
                if let Some(group) = self
                    .groups
                    .iter_mut()
                    .find(|group| group.id == task.resource_group_id)
                {
                    group.runnable_tasks = group.runnable_tasks.saturating_add(1);
                }
            }
            if task.state == TaskState::Ready && task.deadline.is_none() {
                set_ready_priority(&mut self.fair_ready_bitmap, task.effective_priority);
            }
        }
    }
}

fn wait_many_matches(
    items: [(u64, u32); MAX_WAIT_MANY_ITEMS],
    item_count: u8,
    handle: u64,
    signals: u32,
) -> bool {
    items
        .iter()
        .take(item_count as usize)
        .any(|(waited, wanted)| *waited == handle && *wanted & signals != 0)
}

fn set_ready_priority(bitmap: &mut [u64; FAIR_PRIORITY_WORDS], priority: u8) {
    let priority = priority as usize;
    bitmap[priority / 64] |= 1u64 << (priority % 64);
}

fn highest_ready_priority(bitmap: [u64; FAIR_PRIORITY_WORDS]) -> Option<u8> {
    for word_index in (0..FAIR_PRIORITY_WORDS).rev() {
        let word = bitmap[word_index];
        if word != 0 {
            let bit = 63 - word.leading_zeros() as usize;
            return Some((word_index * 64 + bit) as u8);
        }
    }
    None
}

fn utilization_ppm(profile: DeadlineProfile) -> u128 {
    (profile.capacity_ns as u128).saturating_mul(1_000_000) / profile.period_ns as u128
}

fn cpu_mask(cpu_count: u8) -> u64 {
    if cpu_count >= 64 {
        u64::MAX
    } else {
        (1u64 << cpu_count) - 1
    }
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RoundRobinScheduler<const N: usize> {
    tasks: [Option<Task>; N],
    len: usize,
    current: usize,
    ticks: u64,
}

impl<const N: usize> RoundRobinScheduler<N> {
    pub const fn new() -> Self {
        Self {
            tasks: [None; N],
            len: 0,
            current: 0,
            ticks: 0,
        }
    }

    pub fn add_task(&mut self, task: Task) -> Result<(), SchedulerError> {
        if self.len == N {
            return Err(SchedulerError::Full);
        }
        self.tasks[self.len] = Some(task);
        self.len += 1;
        Ok(())
    }

    pub fn current(&self) -> Option<Task> {
        if self.len == 0 {
            None
        } else {
            self.tasks[self.current]
        }
    }

    pub fn tick(&mut self) -> Option<Task> {
        if self.len == 0 {
            return None;
        }
        self.ticks += 1;
        self.current = (self.current + 1) % self.len;
        self.current()
    }

    pub const fn ticks(&self) -> u64 {
        self.ticks
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl<const N: usize> Default for RoundRobinScheduler<N> {
    fn default() -> Self {
        Self::new()
    }
}
use alloc::vec::Vec;

mod runtime;
