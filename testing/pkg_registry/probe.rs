mod font_probe;
use bexos_pkg_client::{ArtifactKind, ArtifactQuery, BlobDigest, HashType, PackageClient};
use bexos_userspace::{Channel, Memory, Startup, log};
use pkg_fidl::{FidlDecode, FidlEncode, HandleRef};
const HOST: &str = "10.0.2.2:18464";
const DIGEST: &[u8; 32] = include_bytes!(env!("APPLICATION_DIGEST"));
const CERT: &[u8] = include_bytes!(env!("CLIENT_CERT"));
const KEY: &[u8] = include_bytes!(env!("CLIENT_KEY"));
pub fn run(channel: u64) -> ! {
    std::panic::set_hook(Box::new(|panic| log(&format!("pkg-probe: {panic}\n"))));
    let startup = Startup::receive(Channel(channel)).unwrap();
    bexos_libc::install_startup(&startup);
    Startup::ready(Channel(channel)).unwrap();
    bexos_userspace::block_on(check(startup));
    bexos_userspace::syscall::exit_with_status(0)
}
fn pkg_call(channel: Channel, ordinal: u64, request: &impl FidlEncode) {
    let mut bytes = [0; 4096];
    let mut handles = [HandleRef { raw: 0 }; 2];
    let encoded = request.encode(&mut bytes, &mut handles).unwrap();
    let raw: Vec<_> = handles[..encoded.handles].iter().map(|h| h.raw).collect();
    let message = bexos_userspace::Rpc(channel)
        .call_raw_with_timeout(ordinal, &bytes[..encoded.bytes], &raw, true, 300)
        .unwrap();
    assert!(message.handles.is_empty());
    let status =
        pkg_fidl::CredentialManagerSetRegistryCredentialResponse::decode(&message.bytes, &[])
            .unwrap()
            .status;
    assert_eq!(status, pkg_fidl::PackageStatus::Ok);
}
async fn check(startup: Startup) {
    let endpoint = |protocol: &str| {
        Channel(
            startup
                .service_grants
                .iter()
                .find(|g| g.protocol == protocol || g.protocol.ends_with(&format!(".{protocol}")))
                .unwrap()
                .endpoint,
        )
    };
    let mut client = PackageClient::from_channel(endpoint("PackageResolver"));
    let digest = BlobDigest {
        hash_type: HashType::Sha256,
        digest: *DIGEST,
    };
    if startup.arg0 == 0 || startup.arg0 == 5 {
        let credentials = endpoint("CredentialManager");
        pkg_call(
            credentials,
            1,
            &pkg_fidl::CredentialManagerSetRegistryCredentialRequest {
                registry_host: HOST,
                auth_token: if startup.arg0 == 5 {
                    "fixture-rotated"
                } else {
                    "fixture-bootstrap"
                },
                sealed_in_trusty: true,
            },
        );
        pkg_call(
            credentials,
            2,
            &pkg_fidl::CredentialManagerSetRegistryMtlsIdentityRequest {
                registry_host: HOST,
                certificate_chain: HandleRef {
                    raw: Memory::from_bytes(CERT).unwrap(),
                },
                certificate_chain_length: CERT.len() as u64,
                private_key: HandleRef {
                    raw: Memory::from_bytes(KEY).unwrap(),
                },
                private_key_length: KEY.len() as u64,
                sealed_in_trusty: true,
            },
        );
        Memory::close(credentials.0).unwrap();
        if startup.arg0 == 5 {
            log("pkg-probe: credentials replaced during pending resolution\n");
            return;
        }
        log("pkg-probe: sealed credentials provisioned\n");
    }
    let query = ArtifactQuery {
        registry_host: HOST.into(),
        repository: "apps/demo".into(),
        tag: "latest".into(),
        expected_digest: Some(digest),
        kind: ArtifactKind::Application,
    };
    if startup.arg0 == 4 {
        assert!(
            startup
                .service_grants
                .iter()
                .all(|g| !g.protocol.ends_with("CredentialManager") || g.endpoint == 0)
        );
        assert!(matches!(
            client.fetch_artifact(&query).await,
            Err(bexos_pkg_client::PackageStatus::AccessDenied)
        ));
        assert!(matches!(
            client.fetch_blob(digest).await,
            Err(bexos_pkg_client::PackageStatus::AccessDenied
                | bexos_pkg_client::PackageStatus::NotFound)
        ));
        log("pkg-probe: unprivileged credentials and cache isolation verified\n");
        return;
    }
    let vmo = if startup.arg0 == 2 {
        client.fetch_blob(digest).await.unwrap()
    } else {
        client.fetch_artifact(&query).await.unwrap()
    };
    if startup.arg0 == 3 {
        log("pkg-probe: pending resolution resumed after replacement\n");
    }
    assert_eq!(vmo.digest().digest, *DIGEST);
    assert!(
        Memory::map(vmo.handle(), vmo.length(), kernel_fidl::Rights::WRITE.0).is_err(),
        "resolver granted writable mapping"
    );
    log("pkg-probe: verified immutable artifact\n");
    if startup.arg0 == 6 {
        log("pkg-probe: pending resolution survived credential replacement\n");
        return;
    }
    let offline = client.fetch_blob(digest).await.unwrap();
    assert_eq!(offline.bytes(), vmo.bytes());
    log("pkg-probe: authorized cached digest verified\n");
    if startup.arg0 != 2 {
        let manager = endpoint("AppManager");
        let mut bytes = [0; 1024];
        let encoded = app_manager_fidl::AppManagerInstallAppFromArtifactRequest {
            query: app_manager_fidl::InstallArtifactQuery {
                registry_host: HOST,
                repository: "apps/demo",
                tag: "latest",
                kind: app_manager_fidl::InstallArtifactKind::Application,
                expected_digest: app_manager_fidl::WireVector::from_slice(&[]),
            },
        }
        .encode(&mut bytes, &mut [])
        .unwrap();
        let message = bexos_userspace::Rpc(manager)
            .call_raw_with_timeout(4, &bytes[..encoded.bytes], &[], true, 300)
            .unwrap();
        assert!(message.handles.is_empty());
        let response =
            app_manager_fidl::AppManagerInstallAppFromArtifactResponse::decode(&message.bytes, &[])
                .unwrap();
        assert_eq!(response.status, app_manager_fidl::AppManagerStatus::Ok);
        assert_eq!(response.allocated_package_id, "bexos.test.pkg_remote");
        Memory::close(manager.0).unwrap();
        log("pkg-probe: appd installed signed OCI application\n");
    }
    if startup.arg0 != 2 {
        font_probe::check(endpoint("FontProvider"), startup.arg0 == 0);
    }
    // Host replacement starts after this marker, while both consumer mappings live.
    log("pkg-probe: VMOs retained for replacement\n");
    let retention = if startup.arg0 == 0 { 900_000 } else { 1_000 };
    let end = bexos_userspace::live_migration::now_ms() + retention;
    let mut checked = bexos_userspace::live_migration::now_ms();
    while bexos_userspace::live_migration::now_ms() < end {
        let now = bexos_userspace::live_migration::now_ms();
        if startup.arg0 == 0 && now.saturating_sub(checked) >= 5_000 {
            use sha2::Digest;
            assert_eq!(offline.bytes(), vmo.bytes());
            assert_eq!(<[u8; 32]>::from(sha2::Sha256::digest(vmo.bytes())), *DIGEST);
            assert!(Memory::map(vmo.handle(), vmo.length(), kernel_fidl::Rights::WRITE.0).is_err());
            log("pkg-probe: original VMOs remain immutable\n");
            checked = now;
        }
        bexos_userspace::yield_now();
    }
    assert_eq!(offline.bytes(), vmo.bytes());
    assert!(Memory::map(vmo.handle(), vmo.length(), kernel_fidl::Rights::WRITE.0).is_err());
    log("pkg-probe: retained VMOs survived\n");
    if startup.arg0 == 2 {
        log("pkg-probe: offline cache acceptance complete\n");
    }
}
use app_manager_fidl::{FidlDecode as AppDecode, FidlEncode as AppEncode};
