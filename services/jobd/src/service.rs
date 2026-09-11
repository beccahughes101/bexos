use alloc::string::{String, ToString};
use alloc::vec::Vec;
use bexos_job_store::{
    ClockSnapshot, JobConstraints, JobRecord, JobRunStatus, JobSpec, JobState, JobTimebase,
    MemoryJobStore, NetworkRequirement, RuntimeConditions, effective_budget,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SchedulerClient {
    pub channel: u64,
    pub package_id: String,
    pub uid: u64,
    pub allowed_ordinals: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunningJob {
    pub token: u64,
    pub package_id: String,
    pub uid: u64,
    pub job_id: String,
    pub started_seconds: u64,
    pub deadline_seconds: u64,
    pub control: u64,
    pub lease: u64,
    pub batch_id: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActiveBatch {
    pub batch_id: u64,
    pub lease: u64,
    pub remaining: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackageDeclarations {
    pub package_id: String,
    pub package_instance_id: u64,
    pub declarations: Vec<JobSpec>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct JobdService {
    pub jobs: MemoryJobStore,
    pub clients: Vec<SchedulerClient>,
    pub running: Vec<RunningJob>,
    pub active_batches: Vec<ActiveBatch>,
    pub system_declarations: Vec<PackageDeclarations>,
    pub locked_users: Vec<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JobdStatus {
    Ok,
    NotFound,
    AccessDenied,
    InvalidArgs,
    Storage,
    ConstraintsUnmet,
    LaunchFailed,
}

impl JobdService {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_declarations(
        &mut self,
        package_id: &str,
        package_instance_id: u64,
        declarations: Vec<JobSpec>,
    ) {
        if let Some(existing) = self
            .system_declarations
            .iter_mut()
            .find(|declaration| declaration.package_id == package_id)
        {
            existing.package_instance_id = package_instance_id;
            existing.declarations = declarations;
        } else {
            self.system_declarations.push(PackageDeclarations {
                package_id: package_id.to_string(),
                package_instance_id,
                declarations,
            });
        }
        self.jobs
            .purge_instance_mismatch(package_id, package_instance_id);
    }

    pub fn package_uninstalled(&mut self, package_id: &str) {
        self.system_declarations
            .retain(|declaration| declaration.package_id != package_id);
        self.running.retain(|job| job.package_id != package_id);
        self.jobs.remove_package(package_id);
    }

    pub fn package_updated(&mut self, package_id: &str, package_instance_id: u64) {
        self.system_declarations
            .retain(|declaration| declaration.package_id != package_id);
        self.jobs
            .purge_instance_mismatch(package_id, package_instance_id);
    }

    pub fn schedule(
        &mut self,
        client: &SchedulerClient,
        spec: JobSpec,
        clocks: ClockSnapshot,
    ) -> Result<JobRecord, JobdStatus> {
        let Some(package) = self.declarations_for(&client.package_id).cloned() else {
            return Err(JobdStatus::AccessDenied);
        };
        self.jobs
            .schedule(
                &client.package_id,
                client.uid,
                package.package_instance_id,
                spec,
                &package.declarations,
                clocks,
            )
            .map_err(store_status)
    }

    pub fn cancel(&mut self, client: &SchedulerClient, job_id: &str) -> JobdStatus {
        match self.jobs.cancel(&client.package_id, client.uid, job_id) {
            Ok(()) => JobdStatus::Ok,
            Err(error) => store_status(error),
        }
    }

    pub fn get(&self, client: &SchedulerClient, job_id: &str) -> Result<JobRecord, JobdStatus> {
        self.jobs
            .get(&client.package_id, client.uid, job_id)
            .cloned()
            .map_err(store_status)
    }

    pub fn list(&self, client: &SchedulerClient) -> Vec<JobRecord> {
        self.jobs.list_for_owner(&client.package_id, client.uid)
    }

    pub fn due_jobs(&self, now_seconds: u64, conditions: RuntimeConditions) -> Vec<JobRecord> {
        self.jobs.due_jobs(now_seconds, conditions)
    }

    pub fn refresh_waiting_states(
        &mut self,
        now_seconds: u64,
        conditions: RuntimeConditions,
    ) -> bool {
        self.jobs.refresh_waiting_states(now_seconds, conditions)
    }

    pub fn mark_user_locked(&mut self, uid: u64, locked: bool) {
        if locked {
            if !self.locked_users.contains(&uid) {
                self.locked_users.push(uid);
            }
        } else {
            self.locked_users.retain(|candidate| *candidate != uid);
        }
        self.jobs.set_user_locked(uid, locked);
    }

    pub fn delete_user(&mut self, uid: u64) {
        self.locked_users.retain(|candidate| *candidate != uid);
        self.running.retain(|job| job.uid != uid);
        self.active_batches.clear();
        self.jobs.remove_user(uid);
    }

    pub fn anchor_realtime_jobs(&mut self, realtime_seconds: u64) {
        self.jobs.anchor_realtime_jobs(realtime_seconds);
    }

    pub fn reconcile_after_boot(&mut self) {
        self.jobs.retain_persistent_only();
        self.jobs.reconcile_after_boot();
    }

    pub fn begin_running(
        &mut self,
        record: &JobRecord,
        now_seconds: u64,
        control: u64,
        lease: u64,
        batch_id: u64,
    ) -> Result<u64, JobdStatus> {
        let token = new_run_token(record, now_seconds);
        self.jobs
            .mark_running(&record.package_id, record.uid, &record.spec.job_id, token)
            .map_err(store_status)?;
        self.running.push(RunningJob {
            token,
            package_id: record.package_id.clone(),
            uid: record.uid,
            job_id: record.spec.job_id.clone(),
            started_seconds: now_seconds,
            deadline_seconds: now_seconds
                .saturating_add(effective_budget(record.spec.max_execution_seconds) as u64),
            control,
            lease,
            batch_id,
        });
        Ok(token)
    }

    pub fn begin_batch(&mut self, lease: u64, expected_jobs: u32) -> u64 {
        let batch_id = self
            .active_batches
            .iter()
            .map(|batch| batch.batch_id)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        self.active_batches.push(ActiveBatch {
            batch_id,
            lease,
            remaining: expected_jobs,
        });
        batch_id
    }

    pub fn finish_batch_job(&mut self, batch_id: u64) -> Option<u64> {
        let batch = self
            .active_batches
            .iter_mut()
            .find(|batch| batch.batch_id == batch_id)?;
        batch.remaining = batch.remaining.saturating_sub(1);
        if batch.remaining != 0 {
            return None;
        }
        let lease = batch.lease;
        self.active_batches
            .retain(|batch| batch.batch_id != batch_id);
        Some(lease)
    }

    pub fn complete(
        &mut self,
        token: u64,
        success: bool,
        reschedule: bool,
        now_seconds: u64,
    ) -> JobdStatus {
        self.running.retain(|job| job.token != token);
        match self
            .jobs
            .complete_token(token, success, reschedule, now_seconds)
        {
            Ok(_) => JobdStatus::Ok,
            Err(error) => store_status(error),
        }
    }

    pub fn expire_timeouts(&mut self, now_seconds: u64) -> Vec<RunningJob> {
        let mut expired = Vec::new();
        self.running.retain(|job| {
            if job.deadline_seconds <= now_seconds {
                let _ = self.jobs.mark_timed_out(job.token, now_seconds);
                expired.push(job.clone());
                false
            } else {
                true
            }
        });
        expired
    }

    fn declarations_for(&self, package_id: &str) -> Option<&PackageDeclarations> {
        self.system_declarations
            .iter()
            .find(|declaration| declaration.package_id == package_id)
    }
}

pub fn manifest_job_to_spec(job: &bexos_appd::JobDefinition) -> JobSpec {
    JobSpec {
        job_id: job.job_id.clone(),
        target_component: job.target_component.clone(),
        initial_delay_seconds: job.initial_delay_seconds,
        interval_seconds: job.interval_seconds,
        flex_window_seconds: job.flex_window_seconds,
        constraints: JobConstraints {
            network: match job.network {
                bexos_appd::JobNetworkConstraint::None => NetworkRequirement::None,
                bexos_appd::JobNetworkConstraint::UnmeteredOnly => NetworkRequirement::Unmetered,
                _ => NetworkRequirement::Any,
            },
            require_charging: job.requires_charging,
            require_device_idle: job.requires_device_idle,
            require_battery_not_low: job.requires_battery_not_low,
        },
        max_execution_seconds: job.max_execution_seconds,
        persist_across_reboots: job.persist_across_reboots,
    }
}

pub fn empty_job_record() -> JobRecord {
    JobRecord {
        package_id: String::new(),
        uid: 0,
        package_instance_id: 0,
        spec: JobSpec::default(),
        state: JobState::Scheduled,
        last_run_status: JobRunStatus::NeverRun,
        timebase: JobTimebase::Monotonic,
        next_run_seconds: 0,
        last_run_seconds: 0,
        job_token: 0,
        run_attempts: 0,
        anchor_delay_seconds: 0,
    }
}

fn new_run_token(record: &JobRecord, now_seconds: u64) -> u64 {
    let mut token = now_seconds ^ record.uid.rotate_left(7);
    for byte in record.package_id.bytes().chain(record.spec.job_id.bytes()) {
        token = token.wrapping_mul(16777619).wrapping_add(byte as u64);
    }
    token.max(1)
}

fn store_status(error: bexos_job_store::JobStoreError) -> JobdStatus {
    match error {
        bexos_job_store::JobStoreError::NotFound => JobdStatus::NotFound,
        bexos_job_store::JobStoreError::UndeclaredJob
        | bexos_job_store::JobStoreError::ConstraintEscalation => JobdStatus::AccessDenied,
        bexos_job_store::JobStoreError::Storage => JobdStatus::Storage,
        _ => JobdStatus::InvalidArgs,
    }
}
