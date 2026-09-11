//! Firmware measurement policy, applied only after the kernel confirms the
//! resident evidence seal and appd verifies the signed platform-policy digest.
use crate::platform_config::{Architecture, PlatformConfig};
use bexos_boot::{
    BOOT_EVIDENCE_FLAG_FIRMWARE_SELECTION, BOOT_EVIDENCE_FLAG_RPMB_ANTI_ROLLBACK,
    BOOT_EVIDENCE_FLAG_SECURE_BOOT, BOOT_EVIDENCE_FLAG_X86_SVM_MONITOR,
    BOOT_EVIDENCE_VERIFIED_MONITOR_VERSION, BootEvidenceV1,
};

pub fn accepts(config: &PlatformConfig, evidence: &BootEvidenceV1) -> bool {
    let policy = &config.tee_policy;
    let required = BOOT_EVIDENCE_FLAG_FIRMWARE_SELECTION
        | BOOT_EVIDENCE_FLAG_RPMB_ANTI_ROLLBACK
        | BOOT_EVIDENCE_FLAG_SECURE_BOOT
        | BOOT_EVIDENCE_FLAG_X86_SVM_MONITOR;
    if policy.allow_authenticated_firmware_selection
        && policy.enforce_secure_boot
        && policy.rpmb_anti_rollback
        && config.metadata.architecture == Architecture::X86_64
        && evidence.version == BOOT_EVIDENCE_VERIFIED_MONITOR_VERSION
        && evidence.flags & required == required
        && evidence.orchestrator_sha256 != [0; 32]
    {
        return true;
    }
    let expected = &policy.orchestrator_verification;
    if expected.is_empty() {
        return !policy.allow_authenticated_firmware_selection;
    }
    let Some(hex) = expected.strip_prefix("sha256:") else {
        return false;
    };
    // Check ASCII before slicing so malformed policy text cannot panic.
    if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return false;
    }
    evidence
        .orchestrator_sha256
        .iter()
        .enumerate()
        .all(|(i, byte)| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16) == Ok(*byte))
}
