use bexos_userspace::live_migration::Source;
use bexos_userspace::{Channel, Startup, log};

use crate::auth::RuntimeUserAuthProvider;
use crate::migration::Runtime;
use crate::service::UsersdService;
#[cfg(feature = "persistent")]
use crate::storage::open_user_store;

pub async fn main(channel: u64) -> ! {
    let control = Channel(channel);
    let startup = Startup::receive(control).expect("usersd startup");
    let state = if startup.migration_target {
        match bexos_userspace::live_migration::receive::<Runtime>(
            control,
            startup.migration_generation,
        ) {
            Ok(mut state) => {
                reopen_persistent_store(&mut state);
                state
            }
            Err(_) => bexos_userspace::exit(),
        }
    } else {
        let vfsd = Channel(
            startup
                .service_grants
                .iter()
                .find(|grant| grant.service == "bexos.service.vfsd")
                .map(|grant| grant.endpoint)
                .unwrap_or(0),
        );
        if vfsd.0 == 0 {
            log("usersd: missing vfsd handle\n");
            bexos_userspace::exit();
        }
        let tee_manager = Channel(
            startup
                .service_grants
                .iter()
                .find(|grant| grant.service == "tee_manager")
                .map(|grant| grant.endpoint)
                .unwrap_or(0),
        );
        let auth = if tee_manager.0 == 0 {
            log("usersd: missing tee_manager handle; auth fails closed\n");
            RuntimeUserAuthProvider::Unsupported
        } else {
            RuntimeUserAuthProvider::connect(tee_manager).unwrap_or_else(|_| {
                log("usersd: Trusty auth provider unavailable; auth fails closed\n");
                RuntimeUserAuthProvider::Unsupported
            })
        };
        let (service, generation) = open_service(vfsd);
        log("usersd: service ready\n");
        Startup::ready(control).unwrap();
        Runtime::new(control, startup.migration, vfsd, service, auth, generation)
    };
    serve(state).await
}

async fn serve(mut runtime: Runtime) -> ! {
    let mut source = Source::new(runtime.migration);
    loop {
        if source.poll(&runtime).is_err() {
            let _ = bexos_userspace::migration::abort();
        }
        if source.quiescing() {
            bexos_userspace::yield_now();
            continue;
        }
        if crate::wire::poll_request(&mut runtime) | crate::wire::poll_bound_requests(&mut runtime)
        {
            source.changed_keys([0, 1, 3, 4, 5]);
        }
        bexos_userspace::yield_now();
    }
}

#[cfg(feature = "persistent")]
fn open_service(vfsd: Channel) -> (UsersdService, u64) {
    match open_user_store(vfsd) {
        Ok(store) => {
            let generation = store_generation(&store);
            (UsersdService::persistent(store), generation)
        }
        Err(_) => {
            log("usersd: awaiting persistent user store
");
            (UsersdService::awaiting_storage(), 0)
        }
    }
}

#[cfg(not(feature = "persistent"))]
fn open_service(vfsd: Channel) -> (UsersdService, u64) {
    let _ = vfsd;
    (UsersdService::new(), 0)
}
pub fn attach_persistent_store(runtime: &mut Runtime) -> Result<(), ()> {
    #[cfg(feature = "persistent")]
    if runtime.service.storage_pending() {
        let store = open_user_store(runtime.vfsd).map_err(|error| {
            log(&alloc::format!(
                "usersd: persistent store initialization failed: {error:?}\n"
            ));
        })?;
        runtime.generation = store_generation(&store);
        runtime.service = UsersdService::persistent(store);
        log("usersd: persistent user store ready\n");
    }
    let _ = runtime;
    Ok(())
}

#[cfg(feature = "persistent")]
fn reopen_persistent_store(runtime: &mut Runtime) {
    runtime.service = match open_user_store(runtime.vfsd) {
        Ok(store) => UsersdService::persistent(store),
        Err(_) => {
            log("usersd: migrated user store failed to reopen\n");
            bexos_userspace::exit();
        }
    };
}

#[cfg(not(feature = "persistent"))]
fn reopen_persistent_store(_runtime: &mut Runtime) {}

#[cfg(feature = "persistent")]
fn store_generation(store: &bexos_user_store::persistent::UserStoreDb) -> u64 {
    store
        .list_users()
        .map(|users| {
            users
                .iter()
                .map(|user| user.record.updated_generation)
                .max()
                .unwrap_or(0)
        })
        .unwrap_or(0)
}
