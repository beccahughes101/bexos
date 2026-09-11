#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetworkRequirement {
    None,
    Any,
    Unmetered,
}

impl NetworkRequirement {
    fn to_u64(self) -> u64 {
        match self {
            Self::None => 1,
            Self::Any => 2,
            Self::Unmetered => 3,
        }
    }

    fn from_u64(value: u64) -> Result<Self, JobStoreError> {
        match value {
            1 => Ok(Self::None),
            2 => Ok(Self::Any),
            3 => Ok(Self::Unmetered),
            _ => Err(JobStoreError::CorruptRecord),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JobState {
    Scheduled,
    Running,
    WaitingConstraints,
    LockedUser,
    Completed,
    Failed,
    Cancelled,
}

impl JobState {
    fn to_u64(self) -> u64 {
        match self {
            Self::Scheduled => 1,
            Self::Running => 2,
            Self::WaitingConstraints => 3,
            Self::LockedUser => 4,
            Self::Completed => 5,
            Self::Failed => 6,
            Self::Cancelled => 7,
        }
    }

    fn from_u64(value: u64) -> Result<Self, JobStoreError> {
        match value {
            1 => Ok(Self::Scheduled),
            2 => Ok(Self::Running),
            3 => Ok(Self::WaitingConstraints),
            4 => Ok(Self::LockedUser),
            5 => Ok(Self::Completed),
            6 => Ok(Self::Failed),
            7 => Ok(Self::Cancelled),
            _ => Err(JobStoreError::CorruptRecord),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JobTimebase {
    Monotonic,
    RealtimeUtc,
    WaitingRealtimeAnchor,
}

impl JobTimebase {
    fn to_u64(self) -> u64 {
        match self {
            Self::Monotonic => 1,
            Self::RealtimeUtc => 2,
            Self::WaitingRealtimeAnchor => 3,
        }
    }

    fn from_u64(value: u64) -> Result<Self, JobStoreError> {
        match value {
            1 => Ok(Self::Monotonic),
            2 => Ok(Self::RealtimeUtc),
            3 => Ok(Self::WaitingRealtimeAnchor),
            _ => Err(JobStoreError::CorruptRecord),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JobRunStatus {
    NeverRun,
    Ok,
    Failed,
    TimedOut,
    Cancelled,
}

impl JobRunStatus {
    fn to_u64(self) -> u64 {
        match self {
            Self::NeverRun => 1,
            Self::Ok => 2,
            Self::Failed => 3,
            Self::TimedOut => 4,
            Self::Cancelled => 5,
        }
    }

    fn from_u64(value: u64) -> Result<Self, JobStoreError> {
        match value {
            1 => Ok(Self::NeverRun),
            2 => Ok(Self::Ok),
            3 => Ok(Self::Failed),
            4 => Ok(Self::TimedOut),
            5 => Ok(Self::Cancelled),
            _ => Err(JobStoreError::CorruptRecord),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JobConstraints {
    pub network: NetworkRequirement,
    pub require_charging: bool,
    pub require_device_idle: bool,
    pub require_battery_not_low: bool,
}

impl Default for JobConstraints {
    fn default() -> Self {
        Self {
            network: NetworkRequirement::Any,
            require_charging: false,
            require_device_idle: false,
            require_battery_not_low: false,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct JobSpec {
    pub job_id: String,
    pub target_component: String,
    pub initial_delay_seconds: u64,
    pub interval_seconds: u64,
    pub flex_window_seconds: u32,
    pub constraints: JobConstraints,
    pub max_execution_seconds: u32,
    pub persist_across_reboots: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JobRecord {
    pub package_id: String,
    pub uid: u64,
    pub package_instance_id: u64,
    pub spec: JobSpec,
    pub state: JobState,
    pub last_run_status: JobRunStatus,
    pub timebase: JobTimebase,
    pub next_run_seconds: u64,
    pub last_run_seconds: u64,
    pub job_token: u64,
    pub run_attempts: u32,
    pub anchor_delay_seconds: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JobStoreError {
    EmptyPackage,
    EmptyJobId,
    EmptyTarget,
    InvalidJobId,
    UndeclaredJob,
    ConstraintEscalation,
    NotFound,
    AlreadyExists,
    CorruptRecord,
    Storage,
    RealtimeUnavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuntimeConditions {
    pub available: bool,
    pub network_available: bool,
    pub network_unmetered: bool,
    pub charging: bool,
    pub device_idle: bool,
    pub battery_low: bool,
    pub thermal_throttled: bool,
}

impl Default for RuntimeConditions {
    fn default() -> Self {
        Self {
            available: true,
            network_available: true,
            network_unmetered: true,
            charging: true,
            device_idle: true,
            battery_low: false,
            thermal_throttled: false,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MemoryJobStore {
    jobs: Vec<JobRecord>,
}

impl MemoryJobStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn list_jobs(&self) -> &[JobRecord] {
        &self.jobs
    }

    pub fn merge_records(&mut self, records: impl IntoIterator<Item = JobRecord>) {
        for record in records {
            self.upsert(record);
        }
    }

    pub fn list_for_owner(&self, package_id: &str, uid: u64) -> Vec<JobRecord> {
        self.jobs
            .iter()
            .filter(|job| job.package_id == package_id && job.uid == uid)
            .cloned()
            .collect()
    }

    pub fn get(
        &self,
        package_id: &str,
        uid: u64,
        job_id: &str,
    ) -> Result<&JobRecord, JobStoreError> {
        self.jobs
            .iter()
            .find(|job| job.package_id == package_id && job.uid == uid && job.spec.job_id == job_id)
            .ok_or(JobStoreError::NotFound)
    }

    pub fn schedule(
        &mut self,
        package_id: &str,
        uid: u64,
        package_instance_id: u64,
        spec: JobSpec,
        declared: &[JobSpec],
        clocks: ClockSnapshot,
    ) -> Result<JobRecord, JobStoreError> {
        validate_package(package_id)?;
        validate_spec(&spec)?;
        let declaration = declared
            .iter()
            .find(|candidate| candidate.job_id == spec.job_id)
            .ok_or(JobStoreError::UndeclaredJob)?;
        validate_subset(&spec, declaration)?;
        let (timebase, next_run_seconds, anchor_delay_seconds) = initial_timebase(&spec, clocks)?;
        let token = next_token(package_id, uid, &spec.job_id, clocks.monotonic_seconds);
        let record = JobRecord {
            package_id: package_id.to_string(),
            uid,
            package_instance_id,
            spec,
            state: JobState::Scheduled,
            last_run_status: JobRunStatus::NeverRun,
            timebase,
            next_run_seconds,
            last_run_seconds: 0,
            job_token: token,
            run_attempts: 0,
            anchor_delay_seconds,
        };
        self.upsert(record.clone());
        Ok(record)
    }

    pub fn cancel(
        &mut self,
        package_id: &str,
        uid: u64,
        job_id: &str,
    ) -> Result<(), JobStoreError> {
        let job = self
            .jobs
            .iter_mut()
            .find(|job| job.package_id == package_id && job.uid == uid && job.spec.job_id == job_id)
            .ok_or(JobStoreError::NotFound)?;
        job.state = JobState::Cancelled;
        job.last_run_status = JobRunStatus::Cancelled;
        Ok(())
    }

    pub fn remove_package(&mut self, package_id: &str) {
        self.jobs.retain(|job| job.package_id != package_id);
    }

    pub fn purge_instance_mismatch(&mut self, package_id: &str, instance_id: u64) {
        self.jobs
            .retain(|job| job.package_id != package_id || job.package_instance_id == instance_id);
    }

    pub fn retain_persistent_only(&mut self) {
        self.jobs.retain(|job| job.spec.persist_across_reboots);
    }

    pub fn reconcile_after_boot(&mut self) {
        for job in &mut self.jobs {
            if matches!(job.state, JobState::Running) {
                job.state = JobState::Scheduled;
                job.job_token = 0;
            }
        }
    }

    pub fn set_user_locked(&mut self, uid: u64, locked: bool) {
        for job in &mut self.jobs {
            if job.uid != uid {
                continue;
            }
            job.state = if locked {
                match job.state {
                    JobState::Running | JobState::Scheduled | JobState::WaitingConstraints => {
                        JobState::LockedUser
                    }
                    other => other,
                }
            } else if matches!(job.state, JobState::LockedUser) {
                JobState::Scheduled
            } else {
                job.state
            };
        }
    }

    pub fn remove_user(&mut self, uid: u64) {
        self.jobs.retain(|job| job.uid != uid);
    }

    pub fn anchor_realtime_jobs(&mut self, realtime_seconds: u64) {
        for job in &mut self.jobs {
            if matches!(job.timebase, JobTimebase::WaitingRealtimeAnchor) {
                job.timebase = JobTimebase::RealtimeUtc;
                job.next_run_seconds = realtime_seconds.saturating_add(job.anchor_delay_seconds);
            }
        }
    }

    pub fn due_jobs(&self, now_seconds: u64, conditions: RuntimeConditions) -> Vec<JobRecord> {
        self.jobs
            .iter()
            .filter(|job| {
                matches!(
                    job.state,
                    JobState::Scheduled | JobState::WaitingConstraints
                ) && !matches!(job.timebase, JobTimebase::WaitingRealtimeAnchor)
                    && job.next_run_seconds
                        <= now_seconds.saturating_add(job.spec.flex_window_seconds as u64)
                    && constraints_met(&job.spec.constraints, conditions)
            })
            .cloned()
            .collect()
    }

    pub fn refresh_waiting_states(
        &mut self,
        now_seconds: u64,
        conditions: RuntimeConditions,
    ) -> bool {
        let mut changed = false;
        for job in &mut self.jobs {
            if !matches!(
                job.state,
                JobState::Scheduled | JobState::WaitingConstraints
            ) {
                continue;
            }
            if job.next_run_seconds
                > now_seconds.saturating_add(job.spec.flex_window_seconds as u64)
                || matches!(job.timebase, JobTimebase::WaitingRealtimeAnchor)
            {
                continue;
            }
            let next_state = if constraints_met(&job.spec.constraints, conditions) {
                JobState::Scheduled
            } else {
                JobState::WaitingConstraints
            };
            if job.state != next_state {
                job.state = next_state;
                changed = true;
            }
        }
        changed
    }

    pub fn mark_running(
        &mut self,
        package_id: &str,
        uid: u64,
        job_id: &str,
        token: u64,
    ) -> Result<(), JobStoreError> {
        let job = self.find_mut(package_id, uid, job_id)?;
        job.state = JobState::Running;
        job.job_token = token;
        job.run_attempts = job.run_attempts.saturating_add(1);
        Ok(())
    }

    pub fn complete_token(
        &mut self,
        token: u64,
        success: bool,
        reschedule: bool,
        now_seconds: u64,
    ) -> Result<JobRecord, JobStoreError> {
        let job = self
            .jobs
            .iter_mut()
            .find(|job| job.job_token == token && job.state == JobState::Running)
            .ok_or(JobStoreError::NotFound)?;
        job.last_run_seconds = now_seconds;
        job.last_run_status = if success {
            JobRunStatus::Ok
        } else {
            JobRunStatus::Failed
        };
        if reschedule && job.spec.interval_seconds != 0 {
            job.state = JobState::Scheduled;
            job.next_run_seconds = next_periodic_deadline(
                job.next_run_seconds,
                job.spec.interval_seconds,
                now_seconds,
            );
        } else {
            job.state = if success {
                JobState::Completed
            } else {
                JobState::Failed
            };
        }
        Ok(job.clone())
    }

    pub fn mark_timed_out(
        &mut self,
        token: u64,
        now_seconds: u64,
    ) -> Result<JobRecord, JobStoreError> {
        let job = self
            .jobs
            .iter_mut()
            .find(|job| job.job_token == token && job.state == JobState::Running)
            .ok_or(JobStoreError::NotFound)?;
        job.last_run_seconds = now_seconds;
        job.last_run_status = JobRunStatus::TimedOut;
        if job.spec.interval_seconds != 0 {
            job.state = JobState::Scheduled;
            job.next_run_seconds = next_periodic_deadline(
                job.next_run_seconds,
                job.spec.interval_seconds,
                now_seconds,
            );
        } else {
            job.state = JobState::Failed;
        }
        Ok(job.clone())
    }

    fn find_mut(
        &mut self,
        package_id: &str,
        uid: u64,
        job_id: &str,
    ) -> Result<&mut JobRecord, JobStoreError> {
        self.jobs
            .iter_mut()
            .find(|job| job.package_id == package_id && job.uid == uid && job.spec.job_id == job_id)
            .ok_or(JobStoreError::NotFound)
    }

    fn upsert(&mut self, record: JobRecord) {
        if let Some(existing) = self.jobs.iter_mut().find(|existing| {
            existing.package_id == record.package_id
                && existing.uid == record.uid
                && existing.spec.job_id == record.spec.job_id
        }) {
            *existing = record;
        } else {
            self.jobs.push(record);
            self.jobs.sort_by(|left, right| {
                (&left.package_id, left.uid, &left.spec.job_id).cmp(&(
                    &right.package_id,
                    right.uid,
                    &right.spec.job_id,
                ))
            });
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ClockSnapshot {
    pub monotonic_seconds: u64,
    pub realtime_seconds: Option<u64>,
}

pub fn constraints_met(constraints: &JobConstraints, conditions: RuntimeConditions) -> bool {
    if !conditions.available {
        return false;
    }
    if conditions.thermal_throttled {
        return false;
    }
    if constraints.require_charging && !conditions.charging {
        return false;
    }
    if constraints.require_device_idle && !conditions.device_idle {
        return false;
    }
    if constraints.require_battery_not_low && conditions.battery_low {
        return false;
    }
    match constraints.network {
        NetworkRequirement::None => true,
        NetworkRequirement::Any => conditions.network_available,
        NetworkRequirement::Unmetered => {
            conditions.network_available && conditions.network_unmetered
        }
    }
}

fn validate_package(package_id: &str) -> Result<(), JobStoreError> {
    if package_id.is_empty() {
        Err(JobStoreError::EmptyPackage)
    } else {
        Ok(())
    }
}

fn validate_spec(spec: &JobSpec) -> Result<(), JobStoreError> {
    if spec.job_id.is_empty() {
        return Err(JobStoreError::EmptyJobId);
    }
    if spec.target_component.is_empty() {
        return Err(JobStoreError::EmptyTarget);
    }
    if spec.job_id.contains('|') || spec.job_id.contains(';') {
        return Err(JobStoreError::InvalidJobId);
    }
    Ok(())
}

fn validate_subset(requested: &JobSpec, declared: &JobSpec) -> Result<(), JobStoreError> {
    if requested.target_component != declared.target_component {
        return Err(JobStoreError::ConstraintEscalation);
    }
    if requested.initial_delay_seconds < declared.initial_delay_seconds {
        return Err(JobStoreError::ConstraintEscalation);
    }
    if requested.interval_seconds != declared.interval_seconds
        && (requested.interval_seconds == 0 || declared.interval_seconds == 0)
    {
        return Err(JobStoreError::ConstraintEscalation);
    }
    if requested.interval_seconds != 0 && requested.interval_seconds < declared.interval_seconds {
        return Err(JobStoreError::ConstraintEscalation);
    }
    if requested.persist_across_reboots && !declared.persist_across_reboots {
        return Err(JobStoreError::ConstraintEscalation);
    }
    if requested.flex_window_seconds < declared.flex_window_seconds {
        return Err(JobStoreError::ConstraintEscalation);
    }
    if !network_subset(requested.constraints.network, declared.constraints.network) {
        return Err(JobStoreError::ConstraintEscalation);
    }
    if declared.constraints.require_charging && !requested.constraints.require_charging {
        return Err(JobStoreError::ConstraintEscalation);
    }
    if declared.constraints.require_device_idle && !requested.constraints.require_device_idle {
        return Err(JobStoreError::ConstraintEscalation);
    }
    if declared.constraints.require_battery_not_low
        && !requested.constraints.require_battery_not_low
    {
        return Err(JobStoreError::ConstraintEscalation);
    }
    if effective_budget(requested.max_execution_seconds)
        > effective_budget(declared.max_execution_seconds)
    {
        return Err(JobStoreError::ConstraintEscalation);
    }
    Ok(())
}

fn network_subset(requested: NetworkRequirement, declared: NetworkRequirement) -> bool {
    network_rank(requested) >= network_rank(declared)
}

const fn network_rank(network: NetworkRequirement) -> u8 {
    match network {
        NetworkRequirement::None => 0,
        NetworkRequirement::Any => 1,
        NetworkRequirement::Unmetered => 2,
    }
}

pub const fn effective_budget(seconds: u32) -> u32 {
    if seconds == 0 { 30 } else { seconds }
}

fn initial_timebase(
    spec: &JobSpec,
    clocks: ClockSnapshot,
) -> Result<(JobTimebase, u64, u64), JobStoreError> {
    if spec.persist_across_reboots {
        if let Some(realtime) = clocks.realtime_seconds {
            Ok((
                JobTimebase::RealtimeUtc,
                realtime.saturating_add(spec.initial_delay_seconds),
                spec.initial_delay_seconds,
            ))
        } else {
            Ok((
                JobTimebase::WaitingRealtimeAnchor,
                0,
                spec.initial_delay_seconds,
            ))
        }
    } else {
        Ok((
            JobTimebase::Monotonic,
            clocks
                .monotonic_seconds
                .saturating_add(spec.initial_delay_seconds),
            spec.initial_delay_seconds,
        ))
    }
}

fn next_periodic_deadline(previous_deadline: u64, interval: u64, now_seconds: u64) -> u64 {
    if interval == 0 {
        return previous_deadline;
    }
    let mut next = previous_deadline.saturating_add(interval);
    if next <= now_seconds {
        let missed = now_seconds.saturating_sub(next) / interval + 1;
        next = next.saturating_add(missed.saturating_mul(interval));
    }
    next
}

fn next_token(package_id: &str, uid: u64, job_id: &str, now_seconds: u64) -> u64 {
    let mut value = now_seconds ^ uid.rotate_left(13);
    for byte in package_id.bytes().chain(job_id.bytes()) {
        value = value.wrapping_mul(1099511628211).wrapping_add(byte as u64);
    }
    value.max(1)
}

pub fn encode_job(record: &JobRecord) -> Vec<u8> {
    let mut out = Vec::new();
    put_string(&mut out, 1, &record.package_id);
    put_field_varint(&mut out, 2, record.uid);
    put_message(&mut out, 3, &encode_spec(&record.spec));
    put_field_varint(&mut out, 4, record.state.to_u64());
    put_field_varint(&mut out, 5, record.last_run_status.to_u64());
    put_field_varint(&mut out, 6, record.next_run_seconds);
    put_field_varint(&mut out, 7, record.last_run_seconds);
    put_field_varint(&mut out, 8, record.job_token);
    put_field_varint(&mut out, 9, record.run_attempts as u64);
    put_field_varint(&mut out, 10, record.timebase.to_u64());
    put_field_varint(&mut out, 11, record.package_instance_id);
    put_field_varint(&mut out, 12, record.anchor_delay_seconds);
    out
}

pub fn decode_job(bytes: &[u8]) -> Result<JobRecord, JobStoreError> {
    let mut record = JobRecord {
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
    };
    read_fields(bytes, |field| {
        match field.number {
            1 => record.package_id = field.string()?,
            2 => record.uid = field.varint()?,
            3 => record.spec = decode_spec(field.bytes()?)?,
            4 => record.state = JobState::from_u64(field.varint()?)?,
            5 => record.last_run_status = JobRunStatus::from_u64(field.varint()?)?,
            6 => record.next_run_seconds = field.varint()?,
            7 => record.last_run_seconds = field.varint()?,
            8 => record.job_token = field.varint()?,
            9 => {
                record.run_attempts =
                    u32::try_from(field.varint()?).map_err(|_| JobStoreError::CorruptRecord)?
            }
            10 => record.timebase = JobTimebase::from_u64(field.varint()?)?,
            11 => record.package_instance_id = field.varint()?,
            12 => record.anchor_delay_seconds = field.varint()?,
            _ => {}
        }
        Ok(())
    })?;
    validate_package(&record.package_id)?;
    validate_spec(&record.spec)?;
    Ok(record)
}

pub fn encode_spec(spec: &JobSpec) -> Vec<u8> {
    let mut out = Vec::new();
    put_string(&mut out, 1, &spec.job_id);
    put_string(&mut out, 2, &spec.target_component);
    put_field_varint(&mut out, 3, spec.initial_delay_seconds);
    put_field_varint(&mut out, 4, spec.interval_seconds);
    put_field_varint(&mut out, 5, spec.flex_window_seconds as u64);
    put_message(&mut out, 6, &encode_constraints(&spec.constraints));
    put_field_varint(&mut out, 7, spec.max_execution_seconds as u64);
    put_field_varint(&mut out, 8, spec.persist_across_reboots as u64);
    out
}

pub fn decode_spec(bytes: &[u8]) -> Result<JobSpec, JobStoreError> {
    let mut spec = JobSpec::default();
    read_fields(bytes, |field| {
        match field.number {
            1 => spec.job_id = field.string()?,
            2 => spec.target_component = field.string()?,
            3 => spec.initial_delay_seconds = field.varint()?,
            4 => spec.interval_seconds = field.varint()?,
            5 => {
                spec.flex_window_seconds =
                    u32::try_from(field.varint()?).map_err(|_| JobStoreError::CorruptRecord)?
            }
            6 => spec.constraints = decode_constraints(field.bytes()?)?,
            7 => {
                spec.max_execution_seconds =
                    u32::try_from(field.varint()?).map_err(|_| JobStoreError::CorruptRecord)?
            }
            8 => spec.persist_across_reboots = field.varint()? != 0,
            _ => {}
        }
        Ok(())
    })?;
    validate_spec(&spec)?;
    Ok(spec)
}

fn encode_constraints(constraints: &JobConstraints) -> Vec<u8> {
    let mut out = Vec::new();
    put_field_varint(&mut out, 1, constraints.network.to_u64());
    put_field_varint(&mut out, 2, constraints.require_charging as u64);
    put_field_varint(&mut out, 3, constraints.require_device_idle as u64);
    put_field_varint(&mut out, 4, constraints.require_battery_not_low as u64);
    out
}

fn decode_constraints(bytes: &[u8]) -> Result<JobConstraints, JobStoreError> {
    let mut constraints = JobConstraints::default();
    read_fields(bytes, |field| {
        match field.number {
            1 => constraints.network = NetworkRequirement::from_u64(field.varint()?)?,
            2 => constraints.require_charging = field.varint()? != 0,
            3 => constraints.require_device_idle = field.varint()? != 0,
            4 => constraints.require_battery_not_low = field.varint()? != 0,
            _ => {}
        }
        Ok(())
    })?;
    Ok(constraints)
}

fn put_message(out: &mut Vec<u8>, field: u64, value: &[u8]) {
    put_key(out, field, 2);
    put_varint(out, value.len() as u64);
    out.extend_from_slice(value);
}

fn put_string(out: &mut Vec<u8>, field: u64, value: &str) {
    put_message(out, field, value.as_bytes());
}

fn put_key(out: &mut Vec<u8>, field: u64, wire: u64) {
    put_varint(out, (field << 3) | wire);
}

fn put_field_varint(out: &mut Vec<u8>, field: u64, value: u64) {
    put_key(out, field, 0);
    put_varint(out, value);
}

fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn read_fields<F>(bytes: &[u8], mut f: F) -> Result<(), JobStoreError>
where
    F: FnMut(Field<'_, '_>) -> Result<(), JobStoreError>,
{
    let mut cursor = Cursor { bytes, pos: 0 };
    while cursor.pos < bytes.len() {
        let key = cursor.varint()?;
        f(Field {
            number: key >> 3,
            wire: (key & 0x7) as u8,
            cursor: &mut cursor,
        })?;
    }
    Ok(())
}

struct Field<'a, 'b> {
    number: u64,
    wire: u8,
    cursor: &'b mut Cursor<'a>,
}

impl<'a, 'b> Field<'a, 'b> {
    fn varint(self) -> Result<u64, JobStoreError> {
        if self.wire != 0 {
            return Err(JobStoreError::CorruptRecord);
        }
        self.cursor.varint()
    }

    fn bytes(self) -> Result<&'a [u8], JobStoreError> {
        if self.wire != 2 {
            return Err(JobStoreError::CorruptRecord);
        }
        let len =
            usize::try_from(self.cursor.varint()?).map_err(|_| JobStoreError::CorruptRecord)?;
        let end = self
            .cursor
            .pos
            .checked_add(len)
            .ok_or(JobStoreError::CorruptRecord)?;
        if end > self.cursor.bytes.len() {
            return Err(JobStoreError::CorruptRecord);
        }
        let out = &self.cursor.bytes[self.cursor.pos..end];
        self.cursor.pos = end;
        Ok(out)
    }

    fn string(self) -> Result<String, JobStoreError> {
        core::str::from_utf8(self.bytes()?)
            .map(str::to_string)
            .map_err(|_| JobStoreError::CorruptRecord)
    }
}

struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Cursor<'_> {
    fn varint(&mut self) -> Result<u64, JobStoreError> {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = *self
                .bytes
                .get(self.pos)
                .ok_or(JobStoreError::CorruptRecord)?;
            self.pos += 1;
            value |= ((byte & 0x7f) as u64) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(JobStoreError::CorruptRecord)
    }
}

#[cfg(feature = "redb_backend")]
pub mod persistent {
    use super::{JobRecord, JobStoreError, MemoryJobStore, decode_job, encode_job};
    use alloc::string::String;
    use alloc::vec::Vec;
    use bexos_redb::{BlockStore, open_or_create_with_store};
    use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};

    const JOBS: TableDefinition<&str, &[u8]> = TableDefinition::new("jobs");

    pub struct JobStoreDb {
        db: Database,
    }

    impl JobStoreDb {
        pub fn open<S: BlockStore>(store: S) -> Result<Self, JobStoreError> {
            Ok(Self {
                db: open_or_create_with_store(store).map_err(|_| JobStoreError::Storage)?,
            })
        }

        pub fn put(&self, record: &JobRecord) -> Result<(), JobStoreError> {
            let tx = self.db.begin_write().map_err(|_| JobStoreError::Storage)?;
            {
                let mut table = tx.open_table(JOBS).map_err(|_| JobStoreError::Storage)?;
                table
                    .insert(job_key(record).as_str(), encode_job(record).as_slice())
                    .map_err(|_| JobStoreError::Storage)?;
            }
            tx.commit().map_err(|_| JobStoreError::Storage)
        }

        pub fn remove(
            &self,
            package_id: &str,
            uid: u64,
            job_id: &str,
        ) -> Result<(), JobStoreError> {
            let tx = self.db.begin_write().map_err(|_| JobStoreError::Storage)?;
            {
                let mut table = tx.open_table(JOBS).map_err(|_| JobStoreError::Storage)?;
                table
                    .remove(key(package_id, uid, job_id).as_str())
                    .map_err(|_| JobStoreError::Storage)?;
            }
            tx.commit().map_err(|_| JobStoreError::Storage)
        }

        pub fn list(&self) -> Result<Vec<JobRecord>, JobStoreError> {
            let tx = self.db.begin_read().map_err(|_| JobStoreError::Storage)?;
            let Ok(table) = tx.open_table(JOBS) else {
                return Ok(Vec::new());
            };
            table
                .iter()
                .map_err(|_| JobStoreError::Storage)?
                .map(|entry| {
                    let (_, value) = entry.map_err(|_| JobStoreError::Storage)?;
                    decode_job(value.value())
                })
                .collect()
        }

        pub fn snapshot_memory(&self) -> Result<MemoryJobStore, JobStoreError> {
            let mut memory = MemoryJobStore::new();
            for record in self.list()? {
                memory.upsert(record);
            }
            Ok(memory)
        }

        pub fn replace_from_memory(&self, memory: &MemoryJobStore) -> Result<(), JobStoreError> {
            let tx = self.db.begin_write().map_err(|_| JobStoreError::Storage)?;
            {
                let mut table = tx.open_table(JOBS).map_err(|_| JobStoreError::Storage)?;
                table
                    .retain(|_, _| false)
                    .map_err(|_| JobStoreError::Storage)?;
                for record in memory.list_jobs() {
                    table
                        .insert(job_key(record).as_str(), encode_job(record).as_slice())
                        .map_err(|_| JobStoreError::Storage)?;
                }
            }
            tx.commit().map_err(|_| JobStoreError::Storage)
        }
    }

    fn job_key(record: &JobRecord) -> String {
        key(&record.package_id, record.uid, &record.spec.job_id)
    }

    fn key(package_id: &str, uid: u64, job_id: &str) -> String {
        alloc::format!("{package_id}|{uid}|{job_id}")
    }
}
