//! Protected Trusty execution-owner checkpoint. Decode CPU, peripherals and
//! transport before installing any live state; a checksum is not authority.
use crate::{platform::Platform, transport};
use bexos_secure_monitor::{
    fabric::CheckpointIdentity,
    state_wire::InvalidState,
    svm::{RestorePolicy, Vmcb},
    vcpu::{CpuIdentity, LegacyExtendedState, Registers},
};
use sha2::{Digest, Sha256};

const HEADER: usize = 64;
const CPU_AT: usize = HEADER + Platform::<1>::STATE_BYTES;
const TRANSPORT_AT: usize = CPU_AT + bexos_secure_monitor::vcpu::STATE_BYTES;
const END: usize = TRANSPORT_AT + transport::STATE_BYTES;
pub const STATE_BYTES: usize = END + 32;

pub struct Prepared {
    cpu: Vmcb,
    registers: Registers,
    platform: Platform<1>,
    transport: transport::Prepared,
}
impl Prepared {
    /// Reconnect the shared transport after a complete record has validated.
    /// CPU registers and device owners remain in resident migration storage.
    pub unsafe fn install_transport_only(self) {
        unsafe {
            self.transport.install();
        }
    }

    /// Both domains must remain stopped from preparation through installation.
    pub unsafe fn install(
        self,
        vmcb: &mut Vmcb,
        registers: &mut Registers,
        platform: &mut Platform<1>,
    ) {
        unsafe {
            self.transport.install();
        }
        *vmcb = self.cpu;
        *registers = self.registers;
        *platform = self.platform;
    }

    /// Install decoded software and transport state while retaining the
    /// permanent owner's live hardware VMCB across policy-image replacement.
    pub unsafe fn install_preserving_cpu(
        self,
        registers: &mut Registers,
        platform: &mut Platform<1>,
    ) {
        unsafe {
            self.transport.install();
        }
        *registers = self.registers;
        *platform = self.platform;
    }
}

/// Both domains must be stopped for the entire snapshot.
pub unsafe fn snapshot(
    epoch: u64,
    vmcb: &Vmcb,
    registers: &Registers,
    platform: &Platform<1>,
    output: &mut [u8],
) -> Result<(), InvalidState> {
    if epoch == 0 || output.len() != STATE_BYTES {
        return Err(InvalidState);
    }
    output.fill(0);
    output[..8].copy_from_slice(b"BEXSW001");
    output[8..12].copy_from_slice(&2u32.to_le_bytes());
    output[12..16].copy_from_slice(&1u32.to_le_bytes());
    output[16..24].copy_from_slice(&epoch.to_le_bytes());
    platform.snapshot(
        CheckpointIdentity {
            domain: 1,
            clock_epoch: epoch,
        },
        unsafe { bexos_secure_monitor::clock::now_ns() },
        &mut output[HEADER..CPU_AT],
    )?;
    registers
        .snapshot(
            vmcb,
            CpuIdentity { domain: 1, cpu: 0 },
            &mut output[CPU_AT..TRANSPORT_AT],
        )
        .map_err(|_| InvalidState)?;
    unsafe { transport::snapshot(&mut output[TRANSPORT_AT..END]) }.map_err(|_| InvalidState)?;
    let digest = Sha256::digest(&output[..END]);
    output[END..].copy_from_slice(&digest);
    Ok(())
}

/// The record must originate in exclusively protected memory. Keep both
/// domains stopped until this returns; no guest-supplied record is permitted.
/// Destination objects may be fresh: restoration never borrows their policy.
pub unsafe fn restore_protected(
    epoch: u64,
    input: &[u8],
    vmcb: &mut Vmcb,
    registers: &mut Registers,
    platform: &mut Platform<1>,
) -> Result<(), InvalidState> {
    unsafe {
        prepare_protected(epoch, input)?.install(vmcb, registers, platform);
    }
    Ok(())
}

/// Prepare a complete secure owner without mutating live CPU, device or
/// transport state. Protected provenance and stopped domains are mandatory.
pub unsafe fn prepare_protected(epoch: u64, input: &[u8]) -> Result<Prepared, InvalidState> {
    // Materialize the page-aligned scratch before any early return. With the
    // pinned compiler, sinking its frame setup past the envelope checks emits
    // an invalid prologue (and misaligns the nested FXSAVE). Keep the scratch
    // address observable at entry; the guest rejection probe exercises this.
    let mut next_cpu = Vmcb::new();
    core::hint::black_box(&mut next_cpu);
    if epoch == 0 || input.len() != STATE_BYTES {
        return Err(InvalidState);
    }
    if &input[..8] != b"BEXSW001"
        || input[8..12] != 2u32.to_le_bytes()
        || input[12..16] != 1u32.to_le_bytes()
        || input[16..24] != epoch.to_le_bytes()
        || input[24..HEADER] != [0; HEADER - 24]
        || input[END..] != Sha256::digest(&input[..END])[..]
    {
        return Err(InvalidState);
    }
    let base = u64::from_le_bytes(input[HEADER + 32..HEADER + 40].try_into().unwrap());
    #[cfg(all(feature = "resident_nucleus", feature = "secure_product"))]
    if !matches!(base, crate::BANK | crate::trusty_owner::SECOND_BANK) {
        return Err(InvalidState);
    }
    #[cfg(not(all(feature = "resident_nucleus", feature = "secure_product")))]
    if base != crate::BANK {
        return Err(InvalidState);
    }
    #[cfg(all(feature = "resident_nucleus", feature = "secure_product"))]
    let root = crate::trusty_owner::table_root(base).ok_or(InvalidState)?;
    #[cfg(not(all(feature = "resident_nucleus", feature = "secure_product")))]
    let root =
        unsafe { (&*core::ptr::addr_of!(crate::TABLES)).root() }.map_err(|_| InvalidState)?;
    let mut next_platform = Platform::new(
        crate::memory::DomainMemory {
            base,
            length: crate::BANK_SIZE,
        },
        true,
    );
    next_platform.restore_protected(
        CheckpointIdentity {
            domain: 1,
            clock_epoch: epoch,
        },
        unsafe { bexos_secure_monitor::clock::now_ns() },
        &input[HEADER..CPU_AT],
    )?;
    let mut next_registers = Registers::default();
    next_registers
        .restore_protected(
            &mut next_cpu,
            CpuIdentity { domain: 1, cpu: 0 },
            RestorePolicy {
                asid: 1,
                npt: root,
                iopm: core::ptr::addr_of!(crate::IOPM) as u64,
                msrpm: core::ptr::addr_of!(crate::MSRPM) as u64,
                mxcsr_mask: LegacyExtendedState::supported_mxcsr_mask(),
            },
            &input[CPU_AT..TRANSPORT_AT],
        )
        .map_err(|_| InvalidState)?;
    let transport = unsafe { transport::prepare_protected(&input[TRANSPORT_AT..END]) }
        .map_err(|_| InvalidState)?;
    Ok(Prepared {
        cpu: next_cpu,
        registers: next_registers,
        platform: next_platform,
        transport,
    })
}

#[cfg(any(feature = "checkpoint_probe", feature = "normal_checkpoint_probe"))]
pub fn corrupt_transport_for_probe(record: &mut [u8]) {
    assert_eq!(record.len(), STATE_BYTES);
    // Preserve the aggregate checksum so rejection must reach the final nested
    // transport validator, after preparing the CPU and peripheral objects.
    record[END - 1] ^= 1;
    let digest = Sha256::digest(&record[..END]);
    record[END..].copy_from_slice(&digest);
}

#[cfg(any(feature = "checkpoint_probe", feature = "normal_checkpoint_probe"))]
pub unsafe fn assert_unchanged_for_probe(
    record: &[u8],
    vmcb: &Vmcb,
    registers: &Registers,
    platform: &Platform<1>,
) {
    static mut TRANSPORT: [u8; transport::STATE_BYTES] = [0; transport::STATE_BYTES];
    let epoch = u64::from_le_bytes(record[16..24].try_into().unwrap());
    let captured = u64::from_le_bytes(record[HEADER + 24..HEADER + 32].try_into().unwrap());
    let mut devices = [0; Platform::<1>::STATE_BYTES];
    platform
        .snapshot(
            CheckpointIdentity {
                domain: 1,
                clock_epoch: epoch,
            },
            captured,
            &mut devices,
        )
        .unwrap();
    assert_eq!(&devices, &record[HEADER..CPU_AT]);
    let mut cpu = [0; bexos_secure_monitor::vcpu::STATE_BYTES];
    registers
        .snapshot(vmcb, CpuIdentity { domain: 1, cpu: 0 }, &mut cpu)
        .unwrap();
    assert_eq!(&cpu, &record[CPU_AT..TRANSPORT_AT]);
    unsafe {
        let transport = &mut *core::ptr::addr_of_mut!(TRANSPORT);
        transport::snapshot(transport).unwrap();
        assert_eq!(transport.as_slice(), &record[TRANSPORT_AT..END]);
    }
}
