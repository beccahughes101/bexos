use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_userspace::{Channel, live_migration::Resource};

use crate::service::{ActiveBatch, JobdService, PackageDeclarations, RunningJob, SchedulerClient};

pub const CLIENTS: u64 = 1;
pub const RUNNING: u64 = 2;
pub const DECLARATIONS: u64 = 3;
pub const WATCHERS: u64 = 4;
pub const BATCHES: u64 = 5;
pub const JOBS: u64 = 6;

pub struct Runtime {
    pub control: Channel,
    pub migration: Option<Channel>,
    pub service: JobdService,
    pub worker_launcher: Option<Channel>,
    pub vfsd: Option<Channel>,
    pub power: Option<Channel>,
    pub netstack: Option<Channel>,
    pub timed: Option<Channel>,
    pub usersd: Option<Channel>,
    pub power_watcher: Option<Channel>,
    pub link_watcher: Option<Channel>,
    pub time_watcher: Option<Channel>,
    pub user_watcher: Option<Channel>,
    pub package_watcher: Option<Channel>,
    pub clock: bool,
    pub generation: u64,
    pub migration_jobs: Option<alloc::vec::Vec<Option<bexos_job_store::JobRecord>>>,
}

impl Runtime {
    pub fn new(
        control: Channel,
        migration: Option<Channel>,
        service: JobdService,
        worker_launcher: Option<Channel>,
        vfsd: Option<Channel>,
        power: Option<Channel>,
        netstack: Option<Channel>,
        timed: Option<Channel>,
        usersd: Option<Channel>,
        clock: bool,
    ) -> Self {
        Self {
            control,
            migration,
            service,
            worker_launcher,
            vfsd,
            power,
            netstack,
            timed,
            usersd,
            power_watcher: None,
            link_watcher: None,
            time_watcher: None,
            user_watcher: None,
            package_watcher: None,
            clock,
            generation: 0,
            migration_jobs: None,
        }
    }
}

impl bexos_userspace::live_migration::State for Runtime {
    fn empty() -> Self {
        Self::new(
            Channel(0),
            None,
            JobdService::new(),
            None,
            None,
            None,
            None,
            None,
            None,
            false,
        )
    }

    fn keys(&self) -> alloc::vec::Vec<u64> {
        (0..JOBS + self.service.jobs.list_jobs().len() as u64).collect()
    }

    fn encode_record(&self, key: u64) -> Result<Option<alloc::vec::Vec<u8>>, Error> {
        if key >= JOBS {
            return Ok(self
                .service
                .jobs
                .list_jobs()
                .get((key - JOBS) as usize)
                .map(bexos_job_store::encode_job));
        }
        let mut w = Encoder::new();
        match key {
            0 => {
                for value in [
                    self.control.0,
                    self.migration.map_or(0, |channel| channel.0),
                    self.worker_launcher.map_or(0, |channel| channel.0),
                    self.vfsd.map_or(0, |channel| channel.0),
                    self.power.map_or(0, |channel| channel.0),
                    self.netstack.map_or(0, |channel| channel.0),
                    self.timed.map_or(0, |channel| channel.0),
                    self.usersd.map_or(0, |channel| channel.0),
                    self.clock as u64,
                    self.generation,
                ] {
                    w.word(value);
                }
                w.word(2);
                w.word(if cfg!(bexos_arch_x86_64) { 2 } else { 1 });
                w.word(self.service.jobs.list_jobs().len() as u64);
                w.word(self.service.locked_users.len() as u64);
                for uid in &self.service.locked_users {
                    w.word(*uid);
                }
            }
            CLIENTS => {
                w.word(self.service.clients.len() as u64);
                for client in &self.service.clients {
                    w.word(client.channel);
                    w.text(&client.package_id);
                    w.word(client.uid);
                    w.word(client.allowed_ordinals);
                }
            }
            RUNNING => {
                w.word(self.service.running.len() as u64);
                for job in &self.service.running {
                    w.word(job.token);
                    w.text(&job.package_id);
                    w.word(job.uid);
                    w.text(&job.job_id);
                    w.word(job.started_seconds);
                    w.word(job.deadline_seconds);
                    w.word(job.control);
                    w.word(job.lease);
                    w.word(job.batch_id);
                }
            }
            DECLARATIONS => {
                w.word(self.service.system_declarations.len() as u64);
                for declaration in &self.service.system_declarations {
                    w.text(&declaration.package_id);
                    w.word(declaration.package_instance_id);
                    w.word(declaration.declarations.len() as u64);
                    for job in &declaration.declarations {
                        w.bytes(&bexos_job_store::encode_spec(job));
                    }
                }
            }
            WATCHERS => {
                w.word(self.power_watcher.map_or(0, |channel| channel.0));
                w.word(self.link_watcher.map_or(0, |channel| channel.0));
                w.word(self.time_watcher.map_or(0, |channel| channel.0));
                w.word(self.user_watcher.map_or(0, |channel| channel.0));
                w.word(self.package_watcher.map_or(0, |channel| channel.0));
            }
            BATCHES => {
                w.word(self.service.active_batches.len() as u64);
                for batch in &self.service.active_batches {
                    w.word(batch.batch_id);
                    w.word(batch.lease);
                    w.word(u64::from(batch.remaining));
                }
            }
            _ => return Err(Error::InvalidData),
        }
        Ok(Some(w.finish()))
    }

    fn adopt_record(&mut self, key: u64, bytes: Option<&[u8]>) -> Result<(), Error> {
        if key >= JOBS {
            let jobs = self.migration_jobs.as_mut().ok_or(Error::InvalidData)?;
            let index = usize::try_from(key - JOBS).map_err(|_| Error::InvalidData)?;
            if index >= jobs.len() {
                return if bytes.is_none() {
                    Ok(())
                } else {
                    Err(Error::InvalidData)
                };
            }
            jobs[index] = bytes
                .map(bexos_job_store::decode_job)
                .transpose()
                .map_err(|_| Error::InvalidData)?;
            self.service.jobs = bexos_job_store::MemoryJobStore::new();
            self.service
                .jobs
                .merge_records(jobs.iter().flatten().cloned());
            return Ok(());
        }
        let Some(bytes) = bytes else {
            return Err(Error::InvalidData);
        };
        let mut r = Decoder::new(bytes);
        match key {
            0 => {
                self.control = Channel(r.word()?);
                self.migration = nonzero_channel(r.word()?);
                self.worker_launcher = nonzero_channel(r.word()?);
                self.vfsd = nonzero_channel(r.word()?);
                self.power = nonzero_channel(r.word()?);
                self.netstack = nonzero_channel(r.word()?);
                self.timed = nonzero_channel(r.word()?);
                self.usersd = nonzero_channel(r.word()?);
                self.clock = r.flag()?;
                self.generation = r.word()?;
                if bytes.len() == 80 {
                    if cfg!(bexos_arch_x86_64) {
                        return Err(Error::InvalidData);
                    }
                    self.migration_jobs = None;
                } else {
                    if r.word()? != 2 {
                        return Err(Error::UnsupportedVersion);
                    }
                    if r.word()? != if cfg!(bexos_arch_x86_64) { 2 } else { 1 } {
                        return Err(Error::InvalidData);
                    }
                    let count = r.count(4096)?;
                    self.service.locked_users.clear();
                    for _ in 0..r.count(4096)? {
                        self.service.locked_users.push(r.word()?);
                    }
                    let jobs = self.migration_jobs.get_or_insert_with(alloc::vec::Vec::new);
                    jobs.resize_with(count, || None);
                    self.service.jobs = bexos_job_store::MemoryJobStore::new();
                    self.service
                        .jobs
                        .merge_records(jobs.iter().flatten().cloned());
                }
            }
            CLIENTS => {
                self.service.clients.clear();
                for _ in 0..r.count(1024)? {
                    self.service.clients.push(SchedulerClient {
                        channel: r.word()?,
                        package_id: r.text(128)?.into(),
                        uid: r.word()?,
                        allowed_ordinals: r.word()?,
                    });
                }
            }
            RUNNING => {
                self.service.running.clear();
                for _ in 0..r.count(1024)? {
                    self.service.running.push(RunningJob {
                        token: r.word()?,
                        package_id: r.text(128)?.into(),
                        uid: r.word()?,
                        job_id: r.text(64)?.into(),
                        started_seconds: r.word()?,
                        deadline_seconds: r.word()?,
                        control: r.word()?,
                        lease: r.word()?,
                        batch_id: r.word()?,
                    });
                }
            }
            DECLARATIONS => {
                self.service.system_declarations.clear();
                for _ in 0..r.count(1024)? {
                    let package_id = r.text(128)?.into();
                    let package_instance_id = r.word()?;
                    let mut declarations = alloc::vec::Vec::new();
                    for _ in 0..r.count(256)? {
                        declarations.push(
                            bexos_job_store::decode_spec(r.bytes(4096)?)
                                .map_err(|_| Error::InvalidData)?,
                        );
                    }
                    self.service.system_declarations.push(PackageDeclarations {
                        package_id,
                        package_instance_id,
                        declarations,
                    });
                }
            }
            WATCHERS => {
                self.power_watcher = nonzero_channel(r.word()?);
                self.link_watcher = nonzero_channel(r.word()?);
                self.time_watcher = nonzero_channel(r.word()?);
                self.user_watcher = nonzero_channel(r.word()?);
                self.package_watcher = nonzero_channel(r.word()?);
            }
            BATCHES => {
                self.service.active_batches.clear();
                for _ in 0..r.count(1024)? {
                    self.service.active_batches.push(ActiveBatch {
                        batch_id: r.word()?,
                        lease: r.word()?,
                        remaining: r.word()? as u32,
                    });
                }
            }
            _ => return Err(Error::InvalidData),
        }
        r.finish()
    }

    fn validate(&self) -> Result<(), Error> {
        if self.control.0 == 0
            || self.migration_jobs.as_ref().is_some_and(|jobs| {
                jobs.iter().any(Option::is_none)
                    || self.service.jobs.list_jobs().len() != jobs.len()
            })
        {
            return Err(Error::InvalidData);
        }
        Ok(())
    }

    fn resources(&self) -> alloc::vec::Vec<Resource> {
        let mut resources = alloc::vec::Vec::new();
        for handle in [
            self.control.0,
            self.migration.map_or(0, |channel| channel.0),
            self.worker_launcher.map_or(0, |channel| channel.0),
            self.vfsd.map_or(0, |channel| channel.0),
            self.power.map_or(0, |channel| channel.0),
            self.netstack.map_or(0, |channel| channel.0),
            self.timed.map_or(0, |channel| channel.0),
            self.usersd.map_or(0, |channel| channel.0),
            self.power_watcher.map_or(0, |channel| channel.0),
            self.link_watcher.map_or(0, |channel| channel.0),
            self.time_watcher.map_or(0, |channel| channel.0),
            self.user_watcher.map_or(0, |channel| channel.0),
            self.package_watcher.map_or(0, |channel| channel.0),
        ] {
            if handle != 0 {
                resources.push(Resource::Handle(handle));
            }
        }
        resources.extend(
            self.service
                .running
                .iter()
                .flat_map(|job| [job.control, job.lease])
                .filter(|handle| *handle != 0)
                .map(Resource::Handle),
        );
        resources.extend(
            self.service
                .active_batches
                .iter()
                .map(|batch| batch.lease)
                .filter(|handle| *handle != 0)
                .map(Resource::Handle),
        );
        resources.retain(|handle| match handle {
            Resource::Handle(raw) => *raw != 0,
            _ => true,
        });
        resources
    }

    fn activated(&mut self, generation: u64) {
        self.generation = generation;
    }
}

fn nonzero_channel(raw: u64) -> Option<Channel> {
    (raw != 0).then_some(Channel(raw))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bexos_userspace::live_migration::State;

    #[test]
    fn snapshot_preserves_nonpersistent_jobs_and_rejects_missing_or_foreign_records() {
        let mut source = Runtime::empty();
        source.control = Channel(1);
        source.service.locked_users.push(2200);
        let mut job = crate::service::empty_job_record();
        job.package_id = "test.package".into();
        job.spec.job_id = "retained".into();
        job.spec.target_component = "worker".into();
        job.job_token = 123;
        job.run_attempts = 7;
        assert!(!job.spec.persist_across_reboots);
        source.service.jobs.merge_records([job.clone()]);
        let header = source.encode_record(0).unwrap().unwrap();
        let mut target = Runtime::empty();
        target.adopt_record(0, Some(&header)).unwrap();
        assert!(target.validate().is_err());
        for key in source.keys().into_iter().skip(1) {
            target
                .adopt_record(key, source.encode_record(key).unwrap().as_deref())
                .unwrap();
        }
        target.validate().unwrap();
        assert_eq!(target.service.jobs.list_jobs(), &[job]);
        assert_eq!(target.service.locked_users, [2200]);
        target.adopt_record(0, Some(&header)).unwrap();
        target.validate().unwrap();
        assert_eq!(target.service.jobs.list_jobs()[0].job_token, 123);
        let mut foreign = header;
        foreign[88..96].copy_from_slice(&3u64.to_le_bytes());
        assert!(Runtime::empty().adopt_record(0, Some(&foreign)).is_err());
    }
}
