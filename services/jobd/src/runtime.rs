#[cfg(feature = "persistent")]
use bexos_userspace::Rpc;
use bexos_userspace::live_migration::Source;
use bexos_userspace::{Channel, Startup, log};
#[cfg(feature = "persistent")]
use user_manager_fidl::{UserManagerListUsersRequest, UserManagerPublicClient};

use crate::migration::Runtime;
use crate::service::JobdService;

pub async fn main(channel: u64) -> ! {
    let control = Channel(channel);
    let startup = Startup::receive(control).expect("jobd startup");
    let grants = StartupGrants::from_startup(&startup);
    if startup.migration_target {
        match bexos_userspace::live_migration::receive::<Runtime>(
            control,
            startup.migration_generation,
        ) {
            Ok(mut state) => {
                grants.apply_missing(&mut state);
                if state.migration_jobs.is_none() {
                    load_persistent_jobs(&mut state);
                }
                state.migration_jobs = None;
                serve(state).await
            }
            Err(_) => bexos_userspace::exit(),
        }
    }
    Startup::ready(control).unwrap();
    log("jobd: ready\n");
    let mut runtime = Runtime::new(
        control,
        startup.migration,
        JobdService::new(),
        grants.worker_launcher,
        grants.vfsd,
        grants.power,
        grants.netstack,
        grants.scoped_network,
        grants.timed,
        grants.usersd,
        grants.clock,
    );
    load_persistent_jobs(&mut runtime);
    runtime.service.reconcile_after_boot();
    serve(runtime).await
}

async fn serve(mut runtime: Runtime) -> ! {
    let mut source = Source::new(runtime.migration);
    let mut persisted_jobs = persistent_records(&runtime);
    let mut changes = bexos_userspace::live_migration::RecordChanges::default();
    loop {
        changes.poll(&runtime, &mut source);
        if source.poll(&runtime).is_err() {
            let _ = bexos_userspace::migration::abort();
        }
        if source.quiescing() {
            bexos_userspace::yield_now();
            continue;
        }
        if crate::wire::poll(&mut runtime).await {
            runtime.generation = runtime.generation.saturating_add(1);
        }
        let records = persistent_records(&runtime);
        if records != persisted_jobs && persist_persistent_jobs(&runtime) {
            persisted_jobs = records;
        }
        bexos_userspace::yield_now();
    }
}

fn persistent_records(runtime: &Runtime) -> Vec<bexos_job_store::JobRecord> {
    runtime
        .service
        .jobs
        .list_jobs()
        .iter()
        .filter(|job| job.spec.persist_across_reboots)
        .cloned()
        .collect()
}

#[cfg(feature = "persistent")]
fn load_persistent_jobs(runtime: &mut Runtime) {
    let Some(vfsd) = runtime.vfsd else {
        return;
    };
    if let Ok(records) = crate::storage::load_system_jobs(vfsd) {
        runtime.service.jobs.merge_records(records);
    }
    for uid in unlocked_users(runtime) {
        if let Ok(records) = crate::storage::load_user_jobs(vfsd, uid) {
            runtime.service.jobs.merge_records(records);
        }
    }
}

#[cfg(not(feature = "persistent"))]
fn load_persistent_jobs(_runtime: &mut Runtime) {}

#[cfg(feature = "persistent")]
pub(crate) fn load_persistent_user_jobs(runtime: &mut Runtime, uid: u64) {
    let Some(vfsd) = runtime.vfsd else {
        return;
    };
    if uid == 0 {
        if let Ok(records) = crate::storage::load_system_jobs(vfsd) {
            runtime.service.jobs.merge_records(records);
        }
    } else if let Ok(records) = crate::storage::load_user_jobs(vfsd, uid) {
        runtime.service.jobs.merge_records(records);
    }
}

#[cfg(not(feature = "persistent"))]
pub(crate) fn load_persistent_user_jobs(_runtime: &mut Runtime, _uid: u64) {}

#[cfg(feature = "persistent")]
fn persist_persistent_jobs(runtime: &Runtime) -> bool {
    let Some(vfsd) = runtime.vfsd else {
        return false;
    };
    let mut succeeded = true;
    let mut system = bexos_job_store::MemoryJobStore::new();
    system.merge_records(
        runtime
            .service
            .jobs
            .list_jobs()
            .iter()
            .filter(|job| job.uid == 0 && job.spec.persist_across_reboots)
            .cloned(),
    );
    succeeded &= crate::storage::open_system_jobs(vfsd)
        .is_ok_and(|store| store.replace_from_memory(&system).is_ok());
    for uid in unlocked_users(runtime) {
        let mut user = bexos_job_store::MemoryJobStore::new();
        user.merge_records(
            runtime
                .service
                .jobs
                .list_jobs()
                .iter()
                .filter(|job| job.uid == uid && job.spec.persist_across_reboots)
                .cloned(),
        );
        succeeded &= crate::storage::open_user_jobs(vfsd, uid)
            .is_ok_and(|store| store.replace_from_memory(&user).is_ok());
    }
    succeeded
}

#[cfg(not(feature = "persistent"))]
fn persist_persistent_jobs(_runtime: &Runtime) -> bool {
    true
}

#[cfg(feature = "persistent")]
fn unlocked_users(runtime: &Runtime) -> Vec<u64> {
    let Some(usersd) = runtime.usersd else {
        return Vec::new();
    };
    let mut client = UserManagerPublicClient::new(Rpc(usersd));
    let mut request_bytes = [0; 16];
    let mut response_bytes = [0; 65500];
    let mut request_handles = [user_manager_fidl::HandleRef { raw: 0 }; 1];
    let mut response_handles = [user_manager_fidl::HandleRef { raw: 0 }; 1];
    client
        .list_users(
            &UserManagerListUsersRequest {},
            &mut request_bytes,
            &mut request_handles,
            &mut response_bytes,
            &mut response_handles,
        )
        .ok()
        .filter(|response| response.status == user_manager_fidl::UserStatus::Ok)
        .map(|response| {
            let mut users = Vec::new();
            for index in 0..response.users.len() {
                if let Ok(user) = response.users.get(index) {
                    if user.unlocked && user.uid != 0 {
                        users.push(user.uid);
                    }
                }
            }
            users
        })
        .unwrap_or_default()
}

struct StartupGrants {
    worker_launcher: Option<Channel>,
    vfsd: Option<Channel>,
    power: Option<Channel>,
    netstack: Option<Channel>,
    scoped_network: bool,
    timed: Option<Channel>,
    usersd: Option<Channel>,
    clock: bool,
}

impl StartupGrants {
    fn from_startup(startup: &Startup) -> Self {
        let mut grants = Self {
            worker_launcher: None,
            vfsd: None,
            power: None,
            netstack: None,
            scoped_network: false,
            timed: None,
            usersd: None,
            clock: false,
        };
        for grant in &startup.service_grants {
            match grant.service.as_str() {
                "WorkerLauncher" => grants.worker_launcher = Some(Channel(grant.endpoint)),
                "bexos.service.vfsd" => grants.vfsd = Some(Channel(grant.endpoint)),
                "bexos.power.PowerManager" => grants.power = Some(Channel(grant.endpoint)),
                "bexos.net.SocketProvider" => {
                    grants.netstack = Some(Channel(grant.endpoint));
                    grants.scoped_network = true;
                }
                "bexos.net.Netstack" if !grants.scoped_network => {
                    grants.netstack = Some(Channel(grant.endpoint));
                }
                "bexos.time.TimeManager" => grants.timed = Some(Channel(grant.endpoint)),
                "bexos.user.UserManager" => grants.usersd = Some(Channel(grant.endpoint)),
                "bexos.kernel.Clock" => grants.clock = true,
                _ => {}
            }
        }
        grants
    }

    fn apply_missing(self, runtime: &mut Runtime) {
        if runtime.worker_launcher.is_none() {
            runtime.worker_launcher = self.worker_launcher;
        }
        if runtime.vfsd.is_none() {
            runtime.vfsd = self.vfsd;
        }
        if runtime.power.is_none() {
            runtime.power = self.power;
        }
        if self.scoped_network {
            if let Some(previous) = runtime
                .netstack
                .replace(self.netstack.expect("scoped grant"))
            {
                let _ = bexos_userspace::Memory::close(previous.0);
            }
            runtime.scoped_network = true;
        } else if runtime.netstack.is_none() {
            runtime.netstack = self.netstack;
            runtime.scoped_network = false;
        }
        if runtime.timed.is_none() {
            runtime.timed = self.timed;
        }
        if runtime.usersd.is_none() {
            runtime.usersd = self.usersd;
        }
        runtime.clock |= self.clock;
    }
}
