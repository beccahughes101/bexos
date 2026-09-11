use bexos_userspace::Memory;
use bexos_userspace::live_migration::Source;
use bexos_userspace::{Channel, Startup, log};
use keychain_fidl::{
    FidlDecode, HandleRef, KeychainAeadDecryptRequest, KeychainAeadDecryptResponse,
    KeychainAeadEncryptRequest, KeychainAeadEncryptResponse, KeychainDeleteSecretRequest,
    KeychainDeleteSecretResponse, KeychainGenerateKeyRequest, KeychainGenerateKeyResponse,
    KeychainGetSecretRequest, KeychainGetSecretResponse, KeychainSignRequest, KeychainSignResponse,
    KeychainStoreSecretRequest, KeychainStoreSecretResponse,
};

use crate::migration::Runtime;
#[cfg(feature = "persistent")]
use crate::service::TeeKeyMintClient;
use crate::service::{KeychainService, RuntimeHardwareKeyProvider, UnsupportedHardwareKeyProvider};
use crate::wire::{envelope, refs, reply, user_auth_token, user_unlocked};

pub async fn main(channel: u64) -> ! {
    let control = Channel(channel);
    let startup = Startup::receive(control).expect("keychaind startup");
    #[cfg(feature = "persistent")]
    if bexos_crypto::init_from_startup(&startup).is_err() {
        log("keychaind: crypto library linker data missing\n");
        bexos_userspace::exit();
    }
    if startup.migration_target {
        match bexos_userspace::live_migration::receive::<Runtime>(
            control,
            startup.migration_generation,
        ) {
            Ok(state) => serve(state).await,
            Err(_) => bexos_userspace::exit(),
        }
    }
    let lazy = bexos_lazy_service::LazyServiceController::restore(
        bexos_lazy_service::LazyServiceSnapshot {
            generation: startup.lazy_generation,
            idle_timeout_ns: u64::from(startup.lazy_idle_timeout_ms) * 1_000_000,
            ..bexos_lazy_service::LazyServiceSnapshot::default()
        },
    );
    let users = Channel(
        startup
            .service_grants
            .iter()
            .find(|grant| grant.service == "bexos.user.UserManager" && grant.capability == "Public")
            .map(|grant| grant.endpoint)
            .unwrap_or(0),
    );
    let user_auth = Channel(
        startup
            .service_grants
            .iter()
            .find(|grant| {
                grant.service == "bexos.user.UserManager" && grant.capability == "UserAuthBroker"
            })
            .map(|grant| grant.endpoint)
            .unwrap_or(0),
    );
    if user_auth.0 == 0 {
        log("keychaind: missing user auth broker handle\n");
        bexos_userspace::exit();
    }
    #[cfg(feature = "persistent")]
    let vfsd = Channel(
        startup
            .service_grants
            .iter()
            .find(|grant| grant.service == "bexos.service.vfsd")
            .map(|grant| grant.endpoint)
            .unwrap_or(0),
    );
    #[cfg(feature = "persistent")]
    if users.0 == 0 || vfsd.0 == 0 {
        log("keychaind: missing startup handles\n");
        bexos_userspace::exit();
    }
    #[cfg(not(feature = "persistent"))]
    if users.0 == 0 {
        log("keychaind: missing users handle\n");
        bexos_userspace::exit();
    }
    #[cfg(feature = "persistent")]
    let (service, volatile_keep_alive) = {
        let teed = Channel(
            startup
                .service_grants
                .iter()
                .find(|grant| grant.service == "tee_manager")
                .map(|grant| grant.endpoint)
                .unwrap_or(0),
        );
        if teed.0 == 0 {
            log("keychaind: missing teed handle\n");
            bexos_userspace::exit();
        }
        let hardware = match TeeKeyMintClient::connect(teed) {
            Ok(client) => RuntimeHardwareKeyProvider::Tee(client),
            Err(_) => {
                log("keychaind: KeyMint unavailable; hardware keys disabled\n");
                RuntimeHardwareKeyProvider::Unsupported(UnsupportedHardwareKeyProvider)
            }
        };
        let service = match crate::service::open_system_keychain(vfsd) {
            Ok(system) => KeychainService::persistent(system, vfsd, hardware),
            Err(_) => {
                log("keychaind: system keychain store unavailable; starting volatile store\n");
                KeychainService::with_hardware(hardware)
            }
        };
        let keep_alive = service.uses_volatile_storage().then(|| lazy.keep_alive());
        (service, keep_alive)
    };
    #[cfg(not(feature = "persistent"))]
    let (service, volatile_keep_alive) = {
        let service = KeychainService::with_hardware(RuntimeHardwareKeyProvider::Unsupported(
            UnsupportedHardwareKeyProvider,
        ));
        let keep_alive = service.uses_volatile_storage().then(|| lazy.keep_alive());
        (service, keep_alive)
    };
    log("keychaind: service ready\n");
    let mut runtime = Runtime::new(
        control,
        startup.migration,
        users,
        user_auth,
        service,
        lazy,
        volatile_keep_alive,
    );
    crate::clients::accept_initial(&mut runtime, &startup.incoming_service_grants);
    Startup::ready(control).unwrap();
    serve(runtime).await
}

async fn serve(mut runtime: Runtime) -> ! {
    let mut source = Source::new(runtime.migration);
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
        crate::clients::poll(&mut runtime);
        poll_lazy_idle(&mut runtime);
        bexos_userspace::yield_now();
    }
}

fn poll_lazy_idle(runtime: &mut Runtime) {
    if runtime.idle_stop_requested.is_some() {
        return;
    }
    let now_ns = bexos_userspace::syscall::ticks().saturating_mul(1_000_000_000)
        / bexos_userspace::syscall::frequency().max(1);
    if let bexos_lazy_service::IdleDecision::Ready { generation } =
        runtime.lazy.idle_decision(now_ns)
    {
        let msg = alloc::format!("bexos.lazy.idle.v1|{generation}");
        if runtime.control.send(msg.as_bytes(), &[]).is_ok() {
            runtime.idle_stop_requested = Some(generation);
        }
    }
}

pub(crate) fn handle_request(
    runtime: &mut Runtime,
    channel: Channel,
    message: bexos_userspace::Message,
) {
    let (ordinal, req) = envelope(&message.bytes);
    let handles = refs(&message.handles);
    match ordinal {
        1 => {
            let q = KeychainStoreSecretRequest::decode(req, &handles).unwrap();
            let user_unlocked = user_unlocked(runtime.users, q.scope, q.uid);
            let hardware_auth_token = user_auth_token(runtime.user_auth, q.scope, q.uid);
            let response = KeychainStoreSecretResponse {
                status: runtime.service.store_secret(
                    q.scope,
                    q.uid,
                    q.alias,
                    &q.secret,
                    q.flags,
                    user_unlocked,
                    hardware_auth_token.as_deref(),
                ),
            };
            reply(channel, &response);
        }
        2 => {
            let q = KeychainGetSecretRequest::decode(req, &handles).unwrap();
            let user_unlocked = user_unlocked(runtime.users, q.scope, q.uid);
            let hardware_auth_token = user_auth_token(runtime.user_auth, q.scope, q.uid);
            let (status, secret) = runtime.service.get_secret(
                q.scope,
                q.uid,
                q.alias,
                user_unlocked,
                hardware_auth_token.as_deref(),
            );
            let response = KeychainGetSecretResponse {
                status,
                secret: &secret,
            };
            reply(channel, &response);
        }
        3 => {
            let q = KeychainDeleteSecretRequest::decode(req, &handles).unwrap();
            let user_unlocked = user_unlocked(runtime.users, q.scope, q.uid);
            let response = KeychainDeleteSecretResponse {
                status: runtime
                    .service
                    .delete_secret(q.scope, q.uid, q.alias, user_unlocked),
            };
            reply(channel, &response);
        }
        4 => {
            let q = KeychainGenerateKeyRequest::decode(req, &handles).unwrap();
            let user_unlocked = user_unlocked(runtime.users, q.scope, q.uid);
            let hardware_auth_token = user_auth_token(runtime.user_auth, q.scope, q.uid);
            let (status, public_key) = runtime.service.generate_key(
                q.scope,
                q.uid,
                q.alias,
                q.algorithm,
                q.flags,
                user_unlocked,
                hardware_auth_token.as_deref(),
            );
            let response = KeychainGenerateKeyResponse {
                status,
                public_key: &public_key,
            };
            reply(channel, &response);
        }
        5 => {
            let q = KeychainSignRequest::decode(req, &handles).unwrap();
            let user_unlocked = user_unlocked(runtime.users, q.scope, q.uid);
            let hardware_auth_token = user_auth_token(runtime.user_auth, q.scope, q.uid);
            let (status, signature) = runtime.service.sign(
                q.scope,
                q.uid,
                q.alias,
                &q.digest,
                user_unlocked,
                hardware_auth_token.as_deref(),
            );
            let response = KeychainSignResponse {
                status,
                signature: &signature,
            };
            reply(channel, &response);
        }
        6 => {
            let q = KeychainAeadEncryptRequest::decode(req, &handles).unwrap();
            let plaintext = read_vmo(q.plaintext.raw, q.plaintext_len);
            let aad = read_vmo(q.aad.raw, q.aad_len);
            let user_unlocked = user_unlocked(runtime.users, q.scope, q.uid);
            let hardware_auth_token = user_auth_token(runtime.user_auth, q.scope, q.uid);
            let (status, nonce, ciphertext) = match (plaintext, aad) {
                (Ok(plaintext), Ok(aad)) => runtime.service.aead_encrypt(
                    q.scope,
                    q.uid,
                    q.alias,
                    &plaintext,
                    &aad,
                    user_unlocked,
                    hardware_auth_token.as_deref(),
                ),
                _ => (
                    keychain_fidl::KeychainStatus::ErrInvalidArgs,
                    [0; 12],
                    Vec::new(),
                ),
            };
            let (ciphertext, ciphertext_len) = vmo_from_bytes_if_ok(status, &ciphertext);
            let response = KeychainAeadEncryptResponse {
                status,
                nonce: &nonce,
                ciphertext: HandleRef { raw: ciphertext },
                ciphertext_len,
            };
            reply(channel, &response);
        }
        7 => {
            let q = KeychainAeadDecryptRequest::decode(req, &handles).unwrap();
            let ciphertext = read_vmo(q.ciphertext.raw, q.ciphertext_len);
            let aad = read_vmo(q.aad.raw, q.aad_len);
            let user_unlocked = user_unlocked(runtime.users, q.scope, q.uid);
            let hardware_auth_token = user_auth_token(runtime.user_auth, q.scope, q.uid);
            let (status, plaintext) = match (ciphertext, aad) {
                (Ok(ciphertext), Ok(aad)) => runtime.service.aead_decrypt(
                    q.scope,
                    q.uid,
                    q.alias,
                    &q.nonce,
                    &ciphertext,
                    &aad,
                    user_unlocked,
                    hardware_auth_token.as_deref(),
                ),
                _ => (keychain_fidl::KeychainStatus::ErrInvalidArgs, Vec::new()),
            };
            let (plaintext, plaintext_len) = vmo_from_bytes_if_ok(status, &plaintext);
            let response = KeychainAeadDecryptResponse {
                status,
                plaintext: HandleRef { raw: plaintext },
                plaintext_len,
            };
            reply(channel, &response);
        }
        _ => panic!("unknown keychaind ordinal"),
    }
}

fn read_vmo(handle: u64, len: u64) -> Result<Vec<u8>, ()> {
    if len == 0 {
        let _ = Memory::close(handle);
        return Ok(Vec::new());
    }
    let rounded = len.checked_add(4095).map(|value| value & !4095).ok_or(())?;
    let va = Memory::map(handle, rounded, 2).map_err(|_| ())?;
    let bytes = unsafe { core::slice::from_raw_parts(va as *const u8, len as usize) }.to_vec();
    Memory::unmap(va, rounded).map_err(|_| ())?;
    Memory::close(handle).map_err(|_| ())?;
    Ok(bytes)
}

fn vmo_from_bytes_if_ok(status: keychain_fidl::KeychainStatus, bytes: &[u8]) -> (u64, u64) {
    if status != keychain_fidl::KeychainStatus::Ok {
        return (0, 0);
    }
    Memory::from_bytes(bytes)
        .map(|handle| (handle, bytes.len() as u64))
        .unwrap_or((0, 0))
}
