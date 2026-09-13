//! Guest-only ownership checks against real kernel objects; no registry required.
use bexos_pkg_client::{
    ArtifactKind, ArtifactQuery, BlobDigest, HashType, PackageClient, PendingResolution,
    ReadOnlyVmo,
};
use bexos_userspace::{Channel, Memory, Startup, log};
use kernel_fidl::{Rights, Status};
use std::{
    future::Future,
    task::{Context, Poll, Waker},
};
const SHA: [u8; 32] = [
    0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d, 0xae, 0x22, 0x23,
    0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10, 0xff, 0x61, 0xf2, 0x00, 0x15, 0xad,
];
pub fn run(channel: u64) -> ! {
    std::panic::set_hook(Box::new(|panic| {
        log(&format!("pkg-protocol-probe: {panic}\n"))
    }));
    let startup = Startup::receive(Channel(channel)).unwrap();
    bexos_libc::install_startup(&startup);
    Startup::ready(Channel(channel)).unwrap();
    let digest = BlobDigest {
        hash_type: HashType::Sha256,
        digest: SHA,
    };
    let rights = Rights::READ.0 | Rights::MAP.0 | Rights::TRANSFER.0;
    for (length, wrong_hash, writable) in [
        (0, false, false),
        (257 * 1024 * 1024, false, false),
        (3, true, false),
        (3, false, true),
    ] {
        let source = Memory::from_bytes(b"abc").unwrap();
        let handle =
            Memory::duplicate(source, rights | if writable { Rights::WRITE.0 } else { 0 }).unwrap();
        Memory::close(source).unwrap();
        let expected = BlobDigest {
            digest: if wrong_hash { [0; 32] } else { SHA },
            ..digest
        };
        assert!(ReadOnlyVmo::adopt(handle, length, digest, Some(&expected)).is_err());
        assert!(Memory::object_info(handle).is_err(), "malformed VMO leaked");
    }
    let source = Memory::from_bytes(b"abc").unwrap();
    let handle = Memory::duplicate(source, rights).unwrap();
    Memory::close(source).unwrap();
    let vmo = ReadOnlyVmo::adopt(handle, 3, digest, Some(&digest)).unwrap();
    assert_eq!(vmo.bytes(), b"abc");
    assert!(Memory::map(handle, 3, Rights::WRITE.0).is_err());
    drop(vmo);
    assert!(Memory::object_info(handle).is_err());
    for malformed in [false, true] {
        let (client, server) = Channel::pair().unwrap();
        let (resource, witness) = Channel::pair().unwrap();
        let mut bytes = [0; 1024];
        let mut handles = [pkg_fidl::HandleRef { raw: 0 }; 1];
        let blob = [pkg_fidl::ResolvedBlob {
            data: pkg_fidl::HandleRef { raw: resource.0 },
            content_length: 3,
            verified_digest: digest,
        }];
        let response = pkg_fidl::PackageResolverResolveArtifactResponse {
            status: bexos_pkg_client::PackageStatus::Ok,
            blob: pkg_fidl::WireVector::from_slice(&blob),
        };
        use pkg_fidl::FidlEncode;
        let encoded = response.encode(&mut bytes, &mut handles).unwrap();
        server
            .send(
                if malformed {
                    b"bad"
                } else {
                    &bytes[..encoded.bytes]
                },
                &[resource.0],
            )
            .unwrap();
        let mut pending = PendingResolution::adopt(client, None);
        assert!(pending.poll().is_err());
        assert!(
            matches!(witness.try_recv(), Err(Status::ErrPeerClosed)),
            "malformed reply leaked its resource"
        );
        drop(pending);
        Memory::close(witness.0).unwrap();
        Memory::close(server.0).unwrap();
    }
    let query = ArtifactQuery {
        registry_host: "registry.test".into(),
        repository: "apps/demo".into(),
        tag: "latest".into(),
        expected_digest: None,
        kind: ArtifactKind::Application,
    };
    let (client, server) = Channel::pair().unwrap();
    let pending = PendingResolution::begin(client, &query).unwrap();
    server.try_recv().unwrap();
    drop(pending);
    assert!(matches!(server.try_recv(), Err(Status::ErrPeerClosed)));
    Memory::close(server.0).unwrap();
    let (channel, server) = Channel::pair().unwrap();
    let mut client = PackageClient::from_channel(channel);
    {
        let future = client.fetch_artifact(&query);
        let mut future = std::pin::pin!(future);
        assert!(matches!(
            future
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop())),
            Poll::Pending
        ));
        server.try_recv().unwrap();
    }
    assert!(
        matches!(server.try_recv(), Err(Status::ErrPeerClosed)),
        "cancelled async call retained its binding"
    );
    Memory::close(server.0).unwrap();
    log("pkg-protocol-probe: malformed resources cancellation and drop verified\n");
    bexos_userspace::syscall::exit_with_status(0)
}
