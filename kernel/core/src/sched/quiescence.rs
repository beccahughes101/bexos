//! Scheduler ownership while a service is stopped for handover.
use super::{Scheduler, TaskState};

impl Scheduler {
    pub fn quiesce_process(&mut self, process_id: u64) {
        for task in &mut self.tasks {
            if task.process_id == process_id
                && matches!(
                    task.state,
                    TaskState::Ready | TaskState::Running | TaskState::Blocked
                )
            {
                task.state = TaskState::Quiesced;
            }
        }
        // Remove every source thread before choosing successors on any CPU.
        for cpu in 0..self.current_task_ids.len() {
            if self.current_task_ids[cpu].is_some_and(|id| {
                self.task(id)
                    .is_some_and(|task| task.state == TaskState::Quiesced)
            }) {
                self.current_task_ids[cpu] = None;
            }
        }
        self.rebuild_runnable_indexes();
        for cpu in 0..self.cpu_count() {
            if self.current_task_ids[cpu as usize].is_none() {
                let _ = self.schedule_next_on_cpu(cpu);
            }
        }
    }

    pub fn resume_quiesced_process(&mut self, process_id: u64) {
        for task in &mut self.tasks {
            if task.process_id == process_id && task.state == TaskState::Quiesced {
                task.state = if task.block_reason.is_some() {
                    TaskState::Blocked
                } else {
                    TaskState::Ready
                };
            }
        }
        self.rebuild_runnable_indexes();
        for cpu in 0..self.cpu_count() {
            if self.current_task_ids[cpu as usize].is_none() {
                let _ = self.schedule_next_on_cpu(cpu);
            }
        }
    }
    pub fn stop_process(&mut self, process_id: u64) {
        for task in &mut self.tasks {
            if task.process_id == process_id
                && matches!(
                    task.state,
                    TaskState::Ready
                        | TaskState::Running
                        | TaskState::Blocked
                        | TaskState::Suspended
                )
            {
                task.state = TaskState::Stopped;
            }
        }
        // Remove every source thread before choosing successors on any CPU.
        for cpu in 0..self.current_task_ids.len() {
            if self.current_task_ids[cpu].is_some_and(|id| {
                self.task(id)
                    .is_some_and(|task| task.state == TaskState::Stopped)
            }) {
                self.current_task_ids[cpu] = None;
            }
        }
        self.rebuild_runnable_indexes();
        for cpu in 0..self.cpu_count() {
            if self.current_task_ids[cpu as usize].is_none() {
                let _ = self.schedule_next_on_cpu(cpu);
            }
        }
    }

    pub fn resume_stopped_process(&mut self, process_id: u64) {
        for task in &mut self.tasks {
            if task.process_id == process_id && task.state == TaskState::Stopped {
                task.state = if task.block_reason.is_some() {
                    TaskState::Blocked
                } else {
                    TaskState::Ready
                };
            }
        }
        self.rebuild_runnable_indexes();
        for cpu in 0..self.cpu_count() {
            if self.current_task_ids[cpu as usize].is_none() {
                let _ = self.schedule_next_on_cpu(cpu);
            }
        }
    }
}
