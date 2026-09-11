//! Capability-scoped process status and job-control stops.
use super::*;
use crate::sched::TaskState;
impl<B: Backend> Runtime<B> {
    pub fn process_status(&self, handle: u64) -> Result<(bool, bool, i32, u64)> {
        let Object::Process(id) = self.capability(handle, ADMIN)?.object else {
            return Err(Status::ErrInvalidHandle);
        };
        let process = &self.processes[id];
        let stopped = self.threads.iter().enumerate().any(|(i, t)| {
            t.process == id
                && self
                    .scheduler
                    .task(i as u64 + 1)
                    .is_some_and(|task| task.state == TaskState::Stopped)
        });
        let status = self
            .threads
            .iter()
            .find(|t| t.process == id)
            .map_or(0, |t| t.exit_code);
        Ok((process.exited, stopped, status, id as u64 + 1))
    }
    pub fn set_process_suspended(&mut self, handle: u64, suspended: bool) -> Result<()> {
        let Object::Process(id) = self.capability(handle, ADMIN)?.object else {
            return Err(Status::ErrInvalidHandle);
        };
        if id == self.current
            || self.processes[id].exited
            || self.processes[id].quarantined
            || self
                .handover
                .as_ref()
                .is_some_and(|h| h.source == id || h.target == id)
        {
            return Err(Status::ErrInvalidArgs);
        }
        if suspended {
            self.scheduler.stop_process(id as u64 + 1);
        } else {
            self.scheduler.resume_stopped_process(id as u64 + 1);
        }
        self.changed(META, 0);
        Ok(())
    }
}
