use bexos_boot::*;
use bexos_kernel_core::bootfs::Bootfs;
use ed25519_dalek::{Signer, SigningKey};
use sha2::{Digest, Sha256};
fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    assert!(
        args.len() == 4 || args.len() >= 7,
        "boot_handoff BOOTFS DESCRIPTOR SHELL_LAYOUT DEVICE_PROTOTXT [EVIDENCE KERNEL DEV_PRIVATE_KEY [--secure-boot] [--rpmb] [--secure-monitor]]"
    );
    let len = std::fs::metadata(&args[0]).unwrap().len();
    let device = std::fs::read_to_string(&args[3]).unwrap();
    let max_cpus = parse_max_cpus(&device);
    let x86 = is_x86_device(&device);
    let secure_monitor = args.iter().skip(7).any(|value| value == "--secure-monitor");
    let mut h = if x86 {
        BootHandoff::qemu_x86_64(len, u64::from(max_cpus))
    } else {
        BootHandoff::qemu_default(len, u64::from(max_cpus))
    };
    // There is no firmware framebuffer at image assembly time. Emit the v3
    // prefix understood by existing signed BL33 images; entry adapters upgrade
    // to v5 when they actually discover framebuffer metadata.
    h.version = 3;
    if secure_monitor {
        assert!(x86, "secure monitor handoff is currently x86-specific");
        h.version = BOOT_HANDOFF_VERSION;
        h.secure_monitor_call_header = bexos_secure_monitor_abi::HEADER;
        h.secure_monitor_features = 1;
    }
    let kernel_start = if x86 { 0x0200_0000 } else { KERNEL_START };
    assert!(h.validate(kernel_start, kernel_start + PAGE));
    let bootfs_addr = h.bootfs_addr;
    let handoff_addr = if x86 { 0x0100_0000 } else { HANDOFF_ADDR };
    let evidence_addr = h.boot_evidence_addr;
    let vbmeta_addr = if x86 { 0x07e0_0000 } else { VBMETA_ADDR };
    let bytes: Vec<_> = h.words().iter().flat_map(|v| v.to_le_bytes()).collect();
    std::fs::write(&args[1], bytes).unwrap();
    if args.len() >= 7 {
        let secure_boot = args.iter().skip(7).any(|value| value == "--secure-boot");
        let rpmb = args.iter().skip(7).any(|value| value == "--rpmb");
        let evidence = boot_evidence(
            &args[0],
            &args[3],
            &args[5],
            &args[6],
            secure_boot,
            rpmb,
            secure_monitor,
        );
        let mut bytes = [0; BootEvidenceV1::BYTES];
        evidence.encode(&mut bytes);
        std::fs::write(&args[4], bytes).unwrap();
    }
    std::fs::write(
        &args[2],
        format!(
            "BOOTFS_ADDR=0x{bootfs_addr:x}\nHANDOFF_ADDR=0x{handoff_addr:x}\nBOOT_EVIDENCE_ADDR=0x{evidence_addr:x}\nVBMETA_ADDR=0x{vbmeta_addr:x}\nRAM_MIB={}\nMAX_CPUS={max_cpus}\n",
            if x86 { 1024 } else { (RAM_END - RAM_START) / 1024 / 1024 }
        ),
    )
    .unwrap();
}

fn boot_evidence(
    bootfs: &str,
    device_prototxt: &str,
    kernel: &str,
    private_key: &str,
    secure_boot: bool,
    rpmb: bool,
    secure_monitor: bool,
) -> BootEvidenceV1 {
    let x86 = is_x86_device(&std::fs::read_to_string(device_prototxt).unwrap());
    let flags = evidence_flags(x86, secure_boot, rpmb, secure_monitor)
        .expect("unsupported boot evidence claim");
    let mut evidence = BootEvidenceV1::qemu_dev(
        1,
        flags,
        sha256_file(kernel),
        sha256_file(bootfs),
        bootfs_entry_sha256(bootfs, "/boot/platform.pcfg"),
        sha256_bytes(b"bexos.ta.orchestrator".to_vec()),
    );
    let mut signed = [0; BootEvidenceV1::SIGNED_BYTES];
    evidence.encode_unsigned(&mut signed);
    let signing = SigningKey::from_bytes(&read_private_key(private_key));
    evidence.signature = signing.sign(&signed).to_bytes();
    assert_eq!(signing.verifying_key().to_bytes(), DEV_BOOT_PUBLIC_KEY);
    evidence
}

fn read_private_key(path: &str) -> [u8; 32] {
    let text = std::fs::read_to_string(path).expect("read QEMU development AVB key");
    let text = text.trim();
    assert_eq!(text.len(), 64, "development AVB key must be 32-byte hex");
    let mut key = [0; 32];
    for (index, byte) in key.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16)
            .expect("development AVB key hex");
    }
    key
}

fn sha256_file(path: &str) -> [u8; 32] {
    sha256_bytes(std::fs::read(path).unwrap())
}

fn sha256_bytes(bytes: Vec<u8>) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn bootfs_entry_sha256(path: &str, entry: &str) -> [u8; 32] {
    let bytes = std::fs::read(path).unwrap();
    let bootfs = Bootfs::parse(&bytes).unwrap();
    let entry = bootfs.find(entry).unwrap().expect("platform policy");
    Sha256::digest(entry.bytes).into()
}

fn parse_max_cpus(prototxt: &str) -> u32 {
    for line in prototxt.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("max_cpus:") {
            let max_cpus = value.trim().parse::<u32>().unwrap();
            assert!((1..=MAX_BOOT_CPUS as u32).contains(&max_cpus));
            return max_cpus;
        }
    }
    1
}

fn evidence_flags(
    x86: bool,
    secure_boot: bool,
    rpmb: bool,
    secure_monitor: bool,
) -> Result<u64, &'static str> {
    if secure_monitor && (!x86 || !secure_boot || !rpmb) {
        return Err("x86 secure monitor evidence requires secure boot and RPMB");
    }
    if x86 && (secure_boot || rpmb) && !secure_monitor {
        return Err("BexOS x86 authenticated Trusty boot requires the in-tree secure monitor");
    }
    let mut flags = if x86 {
        0
    } else {
        BOOT_EVIDENCE_FLAG_IOMMU_STRICT
    };
    if secure_boot {
        flags |= BOOT_EVIDENCE_FLAG_SECURE_BOOT;
    }
    if rpmb {
        flags |= BOOT_EVIDENCE_FLAG_RPMB_ANTI_ROLLBACK;
    }
    if secure_monitor {
        flags |= BOOT_EVIDENCE_FLAG_X86_SVM_MONITOR | BOOT_EVIDENCE_FLAG_IOMMU_STRICT;
    }
    Ok(flags)
}

#[cfg(test)]
mod architecture_tests {
    use super::*;

    #[test]
    fn x86_cannot_claim_an_absent_secure_backend() {
        assert!(is_x86_device("architecture : ARCH_X86_64 # Q35"));
        assert!(!is_x86_device("architecture: ARCH_AARCH64"));
        assert!(std::panic::catch_unwind(|| is_x86_device("architecture: ARCH_UNKNOWN")).is_err());
        assert_eq!(evidence_flags(true, false, false, false), Ok(0));
        assert!(evidence_flags(true, true, false, false).is_err());
        assert!(evidence_flags(true, false, true, false).is_err());
        assert!(evidence_flags(true, true, true, false).is_err());
        assert!(evidence_flags(true, true, false, true).is_err());
        assert!(evidence_flags(false, true, true, true).is_err());
        assert_eq!(
            evidence_flags(true, true, true, true),
            Ok(BOOT_EVIDENCE_FLAG_IOMMU_STRICT
                | BOOT_EVIDENCE_FLAG_SECURE_BOOT
                | BOOT_EVIDENCE_FLAG_RPMB_ANTI_ROLLBACK
                | BOOT_EVIDENCE_FLAG_X86_SVM_MONITOR)
        );
        assert_eq!(
            evidence_flags(false, true, true, false),
            Ok(BOOT_EVIDENCE_FLAG_IOMMU_STRICT
                | BOOT_EVIDENCE_FLAG_SECURE_BOOT
                | BOOT_EVIDENCE_FLAG_RPMB_ANTI_ROLLBACK)
        );
    }
}

fn is_x86_device(device: &str) -> bool {
    let architecture = device
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split('#').next()?.split_once(':')?;
            (key.trim() == "architecture").then_some(value.trim())
        })
        .collect::<Vec<_>>();
    match architecture.as_slice() {
        ["ARCH_AARCH64"] => false,
        ["ARCH_X86_64"] => true,
        _ => panic!("device must declare exactly one supported guest architecture"),
    }
}
