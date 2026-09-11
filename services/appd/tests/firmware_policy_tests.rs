use bexos_appd::{Architecture, PlatformConfig, firmware_policy::accepts};
use bexos_boot::*;

fn fixture() -> (PlatformConfig, BootEvidenceV1) {
    let mut config = PlatformConfig::default();
    config.metadata.architecture = Architecture::X86_64;
    config.tee_policy.enforce_secure_boot = true;
    config.tee_policy.rpmb_anti_rollback = true;
    config.tee_policy.allow_authenticated_firmware_selection = true;
    config.tee_policy.orchestrator_verification = format!("sha256:{}", "11".repeat(32));
    let mut evidence =
        BootEvidenceV1::verified_monitor(1, [1; 32], [2; 32], [3; 32], [4; 32], [5; 32]);
    evidence.flags |= BOOT_EVIDENCE_FLAG_FIRMWARE_SELECTION;
    (config, evidence)
}

#[test]
fn x86_selected_firmware_requires_policy_and_resident_authority() {
    let (config, evidence) = fixture();
    assert!(accepts(&config, &evidence));
    for flag in [
        BOOT_EVIDENCE_FLAG_FIRMWARE_SELECTION,
        BOOT_EVIDENCE_FLAG_RPMB_ANTI_ROLLBACK,
        BOOT_EVIDENCE_FLAG_SECURE_BOOT,
        BOOT_EVIDENCE_FLAG_X86_SVM_MONITOR,
    ] {
        let mut altered = evidence;
        altered.flags &= !flag;
        assert!(!accepts(&config, &altered));
    }
    let mut altered = evidence;
    altered.version = BOOT_EVIDENCE_VERSION;
    assert!(!accepts(&config, &altered));
    altered = evidence;
    altered.orchestrator_sha256 = [0; 32];
    assert!(!accepts(&config, &altered));
    for field in 0..4 {
        let mut altered = config.clone();
        match field {
            0 => altered.tee_policy.allow_authenticated_firmware_selection = false,
            1 => altered.tee_policy.enforce_secure_boot = false,
            2 => altered.tee_policy.rpmb_anti_rollback = false,
            _ => altered.metadata.architecture = Architecture::Unspecified,
        }
        assert!(!accepts(&altered, &evidence));
    }
}

#[test]
fn x86_bootstrap_pin_remains_valid_without_selection_authority() {
    let (mut config, mut evidence) = fixture();
    evidence.flags &= !BOOT_EVIDENCE_FLAG_FIRMWARE_SELECTION;
    evidence.orchestrator_sha256 = [0x11; 32];
    assert!(accepts(&config, &evidence));
    config.tee_policy.orchestrator_verification.clear();
    assert!(!accepts(&config, &evidence));
    config.tee_policy.orchestrator_verification = format!("sha256:{}", "é".repeat(32));
    assert!(!accepts(&config, &evidence));
}

#[test]
fn x86_selection_policy_is_explicit_and_decodes_from_proto() {
    assert!(
        !PlatformConfig::default()
            .tee_policy
            .allow_authenticated_firmware_selection
    );
    let config = PlatformConfig::decode(&[0x1a, 2, 0x38, 1]).unwrap();
    assert!(config.tee_policy.allow_authenticated_firmware_selection);
}
