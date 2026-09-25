use super::*;
use alloc::vec;
const NATIVE: &[u8] = &[0x1a, 5, 0x12, 3, b'e', b'l', b'f'];
#[test]
fn omitted_defaults_to_portable() {
    assert_eq!(
        ManifestArchitecture::decode(&[]).unwrap().validate(None),
        Ok(Architecture::Multi)
    );
}
#[test]
fn native_requires_explicit_architecture() {
    assert_eq!(
        ManifestArchitecture::decode(NATIVE).unwrap().validate(None),
        Err(Error::NativeArchitectureRequired)
    );
}
#[test]
fn stamping_preserves_and_validates_guest_selection() {
    for arch in [Architecture::Aarch64, Architecture::X86_64] {
        let bytes = stamp(NATIVE, arch).unwrap();
        assert!(bytes.starts_with(NATIVE));
        assert_eq!(
            ManifestArchitecture::decode(&bytes)
                .unwrap()
                .validate(Some(arch)),
            Ok(arch)
        );
        let other = if arch == Architecture::Aarch64 {
            Architecture::X86_64
        } else {
            Architecture::Aarch64
        };
        assert_eq!(
            ManifestArchitecture::decode(&bytes)
                .unwrap()
                .validate(Some(other)),
            Err(Error::IncompatibleArchitecture)
        );
        assert_eq!(stamp(&bytes, other), Err(Error::ConflictingArchitecture));
    }
}
#[test]
fn explicit_multi_cannot_be_overwritten() {
    assert_eq!(
        stamp(&[0xa0, 1, 0], Architecture::Aarch64),
        Err(Error::ConflictingArchitecture)
    );
}
#[test]
fn portable_native_and_unknown_architectures_rejected() {
    assert_eq!(
        stamp(NATIVE, Architecture::Multi),
        Err(Error::NativeArchitectureRequired)
    );
    assert_eq!(
        ManifestArchitecture::decode(&[0xa0, 1, 3]),
        Err(Error::UnknownArchitecture)
    );
    assert_eq!(
        ManifestArchitecture::decode(&[0xa0, 1, 1, 0xa0, 1, 1]),
        Err(Error::Malformed)
    );
}
#[test]
fn malformed_lengths_and_varints_rejected() {
    for bytes in [&[0x1a, 255][..], &[0xa0, 1, 255][..], &[0][..]] {
        assert_eq!(ManifestArchitecture::decode(bytes), Err(Error::Malformed));
    }
}
#[test]
fn elf_payload_must_match_declared_machine() {
    let mut elf = [0u8; 64];
    elf[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    elf[18] = 62;
    assert_eq!(validate_payload(Architecture::X86_64, &elf), Ok(()));
    assert_eq!(
        validate_payload(Architecture::Aarch64, &elf),
        Err(Error::IncompatibleArchitecture)
    );
    assert_eq!(
        validate_payload(Architecture::Multi, &elf),
        Err(Error::NativeArchitectureRequired)
    );
}

#[test]
fn sdk_apps_require_identity_abi_and_transplantable_services() {
    let process = [0x20, 1, 0x4a, 2, 0x08, 1];
    let mut manifest = vec![0x0a, 8];
    manifest.extend_from_slice(b"app.test");
    manifest.extend_from_slice(&[0x1a, process.len() as u8]);
    manifest.extend_from_slice(&process);
    manifest.extend_from_slice(&[0x78, 1]);
    assert_eq!(
        validate_sdk_app(&manifest, "app.test", 1),
        Ok(SdkAppMetadata {
            package_name: "app.test".into(),
            min_bexos_abi_version: 1,
        })
    );
    assert_eq!(
        validate_sdk_app(&manifest, "app.other", 1),
        Err(Error::InvalidPackage)
    );
    assert_eq!(
        validate_sdk_app(&manifest, "app.test", 0),
        Err(Error::IncompatibleAbi)
    );

    let mut restarting = manifest.clone();
    restarting[17] = 0;
    assert_eq!(
        validate_sdk_app(&restarting, "app.test", 1),
        Err(Error::MissingHeartTransplant)
    );
}
