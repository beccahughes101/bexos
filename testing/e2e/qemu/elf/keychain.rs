//! Explicit software-secret fixture; this does not request hardware-backed keys.
use bexos_userspace::service_directory::ServiceDirectoryClient;
use bexos_userspace::{Channel, Memory, Rpc, Startup, yield_now};
use keychain_fidl::*;

fn endpoint(startup: &Startup) -> Channel {
    Channel(
        startup
            .service_grants
            .iter()
            .find(|grant| grant.service == "bexos.security.Keychain")
            .expect("keychain fixture grant")
            .endpoint,
    )
}

pub fn store(startup: &Startup) {
    let request = KeychainStoreSecretRequest {
        scope: KeychainScope::System,
        uid: 0,
        alias: "multiarch-retained-client",
        secret: b"keychain value survives service replacement",
        flags: KeyFlags(0),
    };
    let response = call(endpoint(startup), 1, &request);
    assert_eq!(
        KeychainStoreSecretResponse::decode(&response, &[])
            .unwrap()
            .status,
        KeychainStatus::Ok
    );
}

pub fn verify(startup: &Startup) {
    let response = call(
        endpoint(startup),
        2,
        &KeychainGetSecretRequest {
            scope: KeychainScope::System,
            uid: 0,
            alias: "multiarch-retained-client",
        },
    );
    let response = KeychainGetSecretResponse::decode(&response, &[]).unwrap();
    assert_eq!(response.status, KeychainStatus::Ok);
    assert_eq!(
        response.secret,
        b"keychain value survives service replacement"
    );
    bexos_userspace::log("elf-probe: keychain secret and client survived replacement\n");
}

pub fn verify_lazy_activation(startup: &Startup) {
    let startup_keychain = endpoint(startup);
    store_on(
        startup_keychain,
        "lazy-keychain-persistent-secret",
        b"secret stored before idle shutdown",
    );
    let mut directory = ServiceDirectoryClient::from_startup_grants(&startup.service_grants)
        .expect("service directory grant");
    let runtime_keychain = directory
        .connect("bexos.security.Keychain", "Public")
        .expect("runtime keychain connect");
    assert_secret(
        runtime_keychain,
        "lazy-keychain-persistent-secret",
        b"secret stored before idle shutdown",
    );
    bexos_userspace::log("elf-probe: lazy keychain two clients connected\n");
    wait_ms(1_000);
    let _ = Memory::close(runtime_keychain.0);
    let _ = Memory::close(startup_keychain.0);
    bexos_userspace::log("elf-probe: lazy keychain clients released\n");
    wait_ms(8_000);
    let reconnected = directory
        .connect("bexos.security.Keychain", "Public")
        .expect("runtime keychain reconnect");
    assert_secret(
        reconnected,
        "lazy-keychain-persistent-secret",
        b"secret stored before idle shutdown",
    );
    bexos_userspace::log("elf-probe: lazy keychain reconnected after idle stop\n");
    wait_ms(1_000);
    let _ = Memory::close(reconnected.0);
}

fn store_on(channel: Channel, alias: &str, secret: &[u8]) {
    let request = KeychainStoreSecretRequest {
        scope: KeychainScope::System,
        uid: 0,
        alias,
        secret,
        flags: KeyFlags(0),
    };
    let response = call(channel, 1, &request);
    assert_eq!(
        KeychainStoreSecretResponse::decode(&response, &[])
            .unwrap()
            .status,
        KeychainStatus::Ok
    );
}

fn assert_secret(channel: Channel, alias: &str, expected: &[u8]) {
    let response = call(
        channel,
        2,
        &KeychainGetSecretRequest {
            scope: KeychainScope::System,
            uid: 0,
            alias,
        },
    );
    let response = KeychainGetSecretResponse::decode(&response, &[]).unwrap();
    assert_eq!(response.status, KeychainStatus::Ok);
    assert_eq!(response.secret, expected);
}

fn wait_ms(ms: u64) {
    let frequency = bexos_userspace::syscall::frequency();
    let start = bexos_userspace::syscall::ticks();
    let deadline = start.saturating_add(ms.saturating_mul(frequency) / 1_000);
    while bexos_userspace::syscall::ticks() < deadline {
        yield_now();
    }
}

fn call<Q: FidlEncode>(channel: Channel, ordinal: u64, request: &Q) -> Vec<u8> {
    let mut bytes = [0; 512];
    let mut handles = [];
    let encoded = request.encode(&mut bytes, &mut handles).unwrap();
    Rpc(channel)
        .call_raw(ordinal, &bytes[..encoded.bytes], &[], true)
        .expect("keychain fixture RPC")
        .bytes
}
