//! Execution time and deadline budgets use the same monotonic boundaries.
//! Fair scheduling's synthetic charges remain independent.
use super::*;
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Accounting {
    pub last_ns: Option<u64>,
    pub legacy: bool,
    // Retain exited threads: process totals must not decrease on cleanup.
    pub threads: Vec<(u64, u64, u64)>,
}
impl Accounting {
    pub const fn new() -> Self {
        Self {
            last_ns: None,
            legacy: false,
            threads: Vec::new(),
        }
    }
    pub fn register(&mut self, id: u64, process: u64) -> Result<(), SchedulerError> {
        if self.threads.iter().any(|t| t.0 == id) {
            return Err(SchedulerError::AlreadyExists);
        }
        self.threads
            .try_reserve(1)
            .map_err(|_| SchedulerError::Full)?;
        self.threads.push((id, process, 0));
        Ok(())
    }
}
impl Scheduler {
    /// Call at a monotonic scheduling/syscall boundary before changing ownership.
    /// All running CPUs are charged once, independent of which CPU trapped.
    pub fn account_runtime(&mut self, now_ns: u64) {
        let budget_elapsed = match self.runtime.last_ns {
            Some(last) if now_ns < last => return,
            Some(last) => now_ns - last,
            None => now_ns.saturating_sub(self.now_ns),
        };
        let elapsed = match self.runtime.last_ns {
            Some(last) if now_ns < last => return,
            Some(last) => now_ns - last,
            None => 0,
        };
        self.runtime.last_ns = Some(now_ns);
        self.runtime.legacy = false;
        for cpu in 0..self.current_task_ids.len() {
            let Some(id) = self.current_task_ids[cpu] else {
                continue;
            };
            if let Some(t) = self.runtime.threads.iter_mut().find(|t| t.0 == id) {
                t.2 = t.2.saturating_add(elapsed);
            }
            // Yielding and blocking can happen before the timer fires. Charging
            // only timer ticks lets two deadline pollers retain their budgets
            // indefinitely while keeping ordinary I/O services off the CPU.
            if let Some(deadline) = self.task_mut(id).and_then(|task| task.deadline.as_mut()) {
                deadline.remaining_budget_ns =
                    deadline.remaining_budget_ns.saturating_sub(budget_elapsed);
            }
        }
    }
    /// A blocking syscall can already have selected its successor while the
    /// outgoing thread is still executing the dispatcher epilogue.
    pub fn account_executing(&mut self, cpu: u8, thread: u64, now: u64) {
        let Some(current) = self.current_task_ids.get_mut(cpu as usize) else {
            return;
        };
        let saved = *current;
        *current = Some(thread);
        self.account_runtime(now);
        self.current_task_ids[cpu as usize] = saved;
    }
    pub fn runtime_stats(&self, thread_id: u64) -> Option<(u64, u64)> {
        let thread = self.runtime.threads.iter().find(|t| t.0 == thread_id)?;
        let process = self
            .runtime
            .threads
            .iter()
            .filter(|t| t.1 == thread.1)
            .fold(0u64, |sum, t| sum.saturating_add(t.2));
        Some((thread.2, process))
    }
}
