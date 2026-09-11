use bexos_update::{ArtifactKind, TrustedKey, UpdateError, build_signed_manifest, verify_update};

const KEY_ID: [u8; 32] = *b"bexos-qemu-test-ed25519-key-v001";
const SEED: [u8; 32] = [
    0x9d, 0x61, 0xb1, 0x9d, 0xef, 0xfd, 0x5a, 0x60, 0xba, 0x84, 0x4a, 0xf4, 0x92, 0xec, 0x2c, 0xc4,
    0x44, 0x49, 0xc5, 0x69, 0x7b, 0x32, 0x69, 0x19, 0x70, 0x3b, 0xac, 0x03, 0x1c, 0xae, 0x7f, 0x60,
];
const PUBLIC: [u8; 32] = [
    0xd7, 0x5a, 0x98, 0x01, 0x82, 0xb1, 0x0a, 0xb7, 0xd5, 0x4b, 0xfe, 0xd3, 0xc9, 0x64, 0x07, 0x3a,
    0x0e, 0xe1, 0x72, 0xf3, 0xda, 0xa6, 0x23, 0x25, 0xaf, 0x02, 0x1a, 0x68, 0xf7, 0x07, 0x51, 0x1a,
];

fn trusted() -> [TrustedKey<'static>; 1] {
    [TrustedKey {
        key_id: KEY_ID,
        public_key: &PUBLIC,
    }]
}

#[test]
fn verifies_signed_update_manifest() {
    let artifact = b"kernel image bytes";
    let manifest = build_signed_manifest(
        7,
        "kernel",
        ArtifactKind::Microkernel,
        artifact,
        KEY_ID,
        SEED,
    );
    let verified = verify_update(&manifest, artifact, &trusted(), 7).unwrap();
    assert_eq!(verified.manifest.target_id, "kernel");
    assert_eq!(verified.manifest.artifact_kind, ArtifactKind::Microkernel);
}

#[test]
fn rejects_bad_hash_signature_rollback_and_oversized_artifact() {
    let artifact = b"app bundle bytes";
    let manifest =
        build_signed_manifest(7, "app", ArtifactKind::AppPackage, artifact, KEY_ID, SEED);
    assert_eq!(
        verify_update(&manifest, b"changed", &trusted(), 7),
        Err(UpdateError::LengthMismatch)
    );

    let mut bad_signature = manifest.clone();
    bad_signature[120] ^= 0x55;
    assert_eq!(
        verify_update(&bad_signature, artifact, &trusted(), 7),
        Err(UpdateError::BadSignature)
    );

    assert_eq!(
        verify_update(&manifest, artifact, &trusted(), 8),
        Err(UpdateError::Rollback)
    );

    let huge = vec![0xaa; 33 * 1024 * 1024];
    let huge_manifest =
        build_signed_manifest(9, "app", ArtifactKind::AppPackage, &huge, KEY_ID, SEED);
    assert_eq!(
        verify_update(&huge_manifest, &huge, &trusted(), 9),
        Err(UpdateError::ArtifactTooLarge)
    );
}

#[test]
fn hypervisor_signature_binds_component_target_and_entire_manifest() {
    let artifact = b"candidate monitor";
    let manifest = build_signed_manifest(
        8,
        "qemu-x86_64-monitor",
        ArtifactKind::Hypervisor,
        artifact,
        KEY_ID,
        SEED,
    );
    let verified = verify_update(&manifest, artifact, &trusted(), 8).unwrap();
    assert_eq!(verified.manifest.artifact_kind, ArtifactKind::Hypervisor);
    for kind in [ArtifactKind::Microkernel, ArtifactKind::TeeImage] {
        let mut changed = manifest.clone();
        changed[12..16].copy_from_slice(&(kind as u32).to_le_bytes());
        assert_eq!(
            verify_update(&changed, artifact, &trusted(), 8),
            Err(UpdateError::BadSignature)
        );
    }
    let mut changed = manifest.clone();
    *changed.last_mut().unwrap() ^= 1;
    assert_eq!(
        verify_update(&changed, artifact, &trusted(), 8),
        Err(UpdateError::BadSignature)
    );
    let mut appended = manifest.clone();
    appended.push(0);
    assert_eq!(
        verify_update(&appended, artifact, &trusted(), 8),
        Err(UpdateError::LengthMismatch)
    );
    let mut overflowing = manifest;
    overflowing[32..40].copy_from_slice(&u64::MAX.to_le_bytes());
    assert_eq!(
        verify_update(&overflowing, artifact, &trusted(), 8),
        Err(UpdateError::LengthMismatch)
    );
}
