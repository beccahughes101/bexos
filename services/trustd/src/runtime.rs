use alloc::vec::Vec;

use bexos_redb::{RedbStorageBackend, mem::MemBlockStore};
use bexos_trust_store::persistent::TrustStoreDb;
use bexos_userspace::live_migration::Source;
use bexos_userspace::service_binding::{BoundServiceEndpoint, ServiceBinding};
use bexos_userspace::{Channel, Memory, Startup, log};

use crate::TrustdService;
use crate::migration::Runtime;
use crate::wire::{poll_app_clients, poll_tls_clients};

pub async fn main(channel: u64) -> ! {
    let control = Channel(channel);
    let startup = Startup::receive(control).expect("trustd startup");
    if startup.migration_target {
        match bexos_userspace::live_migration::receive::<Runtime>(
            control,
            startup.migration_generation,
        ) {
            Ok(state) => serve(state).await,
            Err(_) => bexos_userspace::exit(),
        }
    }
    if startup.resources.len() != 2 || startup.arg0 == 0 || startup.arg1 == 0 {
        log("trustd: missing root store startup resources\n");
        bexos_userspace::exit();
    }
    let tls_roots_redb =
        read_startup_resource(startup.resources[0], startup.arg0).expect("trustd tls roots map");
    let app_roots_redb =
        read_startup_resource(startup.resources[1], startup.arg1).expect("trustd app roots map");
    let app_roots = match load_app_roots_owned(app_roots_redb) {
        Ok(roots) => roots,
        Err(_) => {
            log("trustd: app-signing root store failed to load\n");
            bexos_userspace::exit();
        }
    };
    let migration = startup.migration;
    startup.close_resources();
    let service = TrustdService::with_tls_roots(app_roots, tls_roots_redb);
    let _ = Startup::ready(control);
    log("trustd: service ready; BootFS root stores available for early validation\n");
    serve(Runtime::new(control, migration, service)).await
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
        if let Ok(message) = runtime.control.try_recv() {
            if let (Some(endpoint), Ok(metadata)) = (
                message.handles.first().copied(),
                core::str::from_utf8(&message.bytes),
            ) {
                if let Some(binding) = ServiceBinding::parse(metadata) {
                    if binding.protocol_is("AppTrustManager") {
                        runtime
                            .app_clients
                            .push(BoundServiceEndpoint::new_with_protocol(
                                Channel(endpoint),
                                binding.method_ordinals,
                                "AppTrustManager",
                            ));
                        source.changed(0);
                    } else if binding.protocol_is("TlsTrustManager") {
                        runtime
                            .tls_clients
                            .push(BoundServiceEndpoint::new_with_protocol(
                                Channel(endpoint),
                                binding.method_ordinals,
                                "TlsTrustManager",
                            ));
                        source.changed(0);
                    }
                }
            }
        }
        if poll_app_clients(&mut runtime.app_clients, &mut runtime.service)
            || poll_tls_clients(&mut runtime.tls_clients, &runtime.service)
        {
            source.changed(0);
        }
        bexos_userspace::yield_now();
    }
}

fn read_startup_resource(handle: u64, len: u64) -> Result<Vec<u8>, kernel_fidl::Status> {
    let map_len = bexos_boot::page_round(len).ok_or(kernel_fidl::Status::ErrInvalidArgs)?;
    let va = Memory::map(handle, map_len, 2)?;
    let len = usize::try_from(len).map_err(|_| kernel_fidl::Status::ErrInvalidArgs)?;
    let mut bytes = Vec::with_capacity(len);
    Memory::commit_range(bytes.as_mut_ptr() as u64, len as u64)?;
    unsafe {
        core::ptr::copy_nonoverlapping(va as *const u8, bytes.as_mut_ptr(), len);
        bytes.set_len(len);
    }
    Memory::unmap(va, map_len)?;
    Ok(bytes)
}

pub fn load_app_roots(
    bytes: &[u8],
) -> Result<Vec<bexos_trust_store::AppSigningRootAnchor>, bexos_trust_store::TrustStoreError> {
    load_app_roots_owned(bytes.to_vec())
}

fn load_app_roots_owned(
    bytes: Vec<u8>,
) -> Result<Vec<bexos_trust_store::AppSigningRootAnchor>, bexos_trust_store::TrustStoreError> {
    let store = MemBlockStore::from_bytes(bytes);
    let backend = RedbStorageBackend::new(store);
    TrustStoreDb::open_with_backend(backend).and_then(|db| db.list_app_roots())
}
