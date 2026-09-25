use super::*;
use alloc::{vec, vec::Vec};
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
            component_type: SdkComponentType::Application,
            boot_wave: None,
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

#[test]
fn sdk_component_roles_and_boot_wave_are_enforced() {
    let process = [0x20, 1, 0x40, 4, 0x4a, 2, 0x08, 1];
    let mut service = vec![0x0a, 12];
    service.extend_from_slice(b"service.test");
    service.extend_from_slice(&[0x1a, process.len() as u8]);
    service.extend_from_slice(&process);
    service.extend_from_slice(&[0x78, 1]);
    assert!(
        validate_sdk_component(
            &service,
            "service.test",
            1,
            SdkComponentType::Service,
            Some(4),
        )
        .is_ok()
    );
    assert_eq!(
        validate_sdk_component(
            &service,
            "service.test",
            1,
            SdkComponentType::Service,
            Some(3),
        ),
        Err(Error::InvalidBootWave)
    );
    assert_eq!(
        validate_sdk_component(
            &service,
            "service.test",
            1,
            SdkComponentType::Driver,
            Some(4),
        ),
        Err(Error::InvalidDriver)
    );
}

fn length_field(number: u8, value: &[u8]) -> Vec<u8> {
    let mut out = vec![(number << 3) | 2, value.len() as u8];
    out.extend_from_slice(value);
    out
}

fn int_field(number: u8, value: u8) -> Vec<u8> {
    vec![number << 3, value]
}

fn valid_driver_manifest(
    include_resource: bool,
    include_bind: bool,
    driver_package: &str,
) -> Vec<u8> {
    let mut process = Vec::new();
    process.extend(length_field(2, b"elf"));
    process.extend(int_field(4, 1));
    process.extend(int_field(8, 4));
    process.extend(length_field(9, &int_field(1, 1)));
    let mut options = Vec::new();
    options.extend(length_field(2, &length_field(1, b"/pkg/bin/driver")));
    process.extend(length_field(7, &options));

    let mut driver = Vec::new();
    driver.extend(length_field(1, b"test_driver"));
    driver.extend(length_field(2, driver_package.as_bytes()));
    driver.extend(length_field(4, &int_field(2, 1)));
    if include_resource {
        let mut resource = int_field(1, 1);
        resource.extend(int_field(2, 1));
        driver.extend(length_field(5, &resource));
    }

    let mut condition = int_field(1, 1);
    condition.extend(length_field(2, &length_field(1, b"pci.vendor_id")));
    let bind = length_field(1, &condition);

    let mut manifest = length_field(1, b"driver.test");
    manifest.extend(length_field(3, &process));
    manifest.extend(length_field(8, &driver));
    if include_bind {
        manifest.extend(length_field(9, &bind));
    }
    manifest.extend(int_field(15, 1));
    manifest
}

#[test]
fn sdk_driver_identity_resources_bind_rules_and_executable_are_enforced() {
    let valid = valid_driver_manifest(true, true, "driver.test");
    assert!(
        validate_sdk_component(&valid, "driver.test", 1, SdkComponentType::Driver, Some(4),)
            .is_ok()
    );
    for invalid in [
        valid_driver_manifest(false, true, "driver.test"),
        valid_driver_manifest(true, false, "driver.test"),
        valid_driver_manifest(true, true, "driver.other"),
    ] {
        assert_eq!(
            validate_sdk_component(
                &invalid,
                "driver.test",
                1,
                SdkComponentType::Driver,
                Some(4),
            ),
            Err(Error::InvalidDriver)
        );
    }

    let mut unsafe_path = valid;
    let position = unsafe_path
        .windows(b"/pkg/bin/driver".len())
        .position(|window| window == b"/pkg/bin/driver")
        .unwrap();
    unsafe_path[position..position + 15].copy_from_slice(b"/boot/bin/bad!!");
    assert_eq!(
        validate_sdk_component_contract(&unsafe_path, 1, SdkComponentType::Driver, Some(4)),
        Err(Error::InvalidPackage)
    );
}
