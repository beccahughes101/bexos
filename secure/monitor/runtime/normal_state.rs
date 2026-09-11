//! Protected aggregate checkpoint of BexOS's four CPUs and virtual platform.
//! The resident owner supplies NPT and device policy. Records never supply host
//! pointers, and validation finishes before any live CPU object is replaced.
use super::*;
use bexos_secure_monitor::{
    fabric::CheckpointIdentity,
    state_wire::InvalidState,
    svm::RestorePolicy,
    vcpu::{CpuIdentity, LegacyExtendedState, STATE_BYTES as CPU_BYTES},
};
use sha2::{Digest, Sha256};

const HEADER: usize = 64;
const CPUS_AT: usize = HEADER + Platform::<4>::STATE_BYTES;
pub const STATE_BYTES: usize = CPUS_AT + 4 * CPU_BYTES + 32;

pub struct Prepared {
    normal: Normal,
    cpus: [Vmcb; 4],
}
impl Prepared {
    /// Both domains must remain stopped until installation completes.
    pub unsafe fn install(self) -> Normal {
        unsafe {
            *core::ptr::addr_of_mut!(CPUS) = self.cpus;
        }
        self.normal
    }
}

impl Normal {
    /// Reconstruct the software owner without borrowing the retiring Normal
    /// object. The caller must provide a stopped domain's protected checkpoint;
    /// a checksum on guest-supplied bytes never supplies boot authority.
    pub unsafe fn resume_protected(epoch: u64, input: &[u8]) -> Result<Self, InvalidState> {
        unsafe { Ok(Self::prepare_protected(epoch, input)?.install()) }
    }

    /// Validate a stopped domain's protected record without changing any live
    /// CPU or device. Dropping this preparation leaves the old owner intact.
    pub unsafe fn prepare_protected(epoch: u64, input: &[u8]) -> Result<Prepared, InvalidState> {
        let root = unsafe { (&*core::ptr::addr_of!(TABLES)).root() }.map_err(|_| InvalidState)?;
        if root != core::ptr::addr_of!(TABLES) as u64 {
            return Err(InvalidState);
        }
        let mut result = Self {
            platform: Platform::for_restore(
                DomainMemory {
                    base: BASE,
                    length: LENGTH,
                },
                false,
            ),
            registers: core::array::from_fn(|_| Registers::default()),
            root,
            next: 0,
            // This temporary object is never entered before all protected
            // fields, including the original approval bit, validate below.
            entry_approved: true,
            #[cfg(feature = "secure_product")]
            evidence_address: bexos_boot::BootHandoff::qemu_x86_64(0, 4).boot_evidence_addr
                as usize,
        };
        let cpus = unsafe { result.decode_record(epoch, input)? };
        Ok(Prepared {
            normal: result,
            cpus,
        })
    }
    pub unsafe fn snapshot(&self, epoch: u64, output: &mut [u8]) -> Result<(), InvalidState> {
        unsafe { self.snapshot_at(epoch, bexos_secure_monitor::clock::now_ns(), output) }
    }

    unsafe fn snapshot_at(
        &self,
        epoch: u64,
        now: u64,
        output: &mut [u8],
    ) -> Result<(), InvalidState> {
        if output.len() != STATE_BYTES || epoch == 0 || !self.entry_approved || self.next >= 4 {
            return Err(InvalidState);
        }
        output.fill(0);
        output[..8].copy_from_slice(b"BEXNW001");
        output[8..12].copy_from_slice(&2u32.to_le_bytes());
        output[12..16].copy_from_slice(&4u32.to_le_bytes());
        output[16..24].copy_from_slice(&epoch.to_le_bytes());
        output[24..32].copy_from_slice(&BASE.to_le_bytes());
        output[32..40].copy_from_slice(&(LENGTH as u64).to_le_bytes());
        output[40] = self.next as u8;
        output[41] = 1;
        #[cfg(feature = "secure_product")]
        output[48..56].copy_from_slice(&(self.evidence_address as u64).to_le_bytes());
        self.platform.snapshot(
            CheckpointIdentity {
                domain: 2,
                clock_epoch: epoch,
            },
            now,
            &mut output[HEADER..CPUS_AT],
        )?;
        for id in 0..4 {
            if self.platform.devices.fabric.state(id) != Some(CpuState::Running) {
                continue;
            }
            output[42] |= 1 << id;
            let cpu = unsafe { &(*core::ptr::addr_of!(CPUS))[id] };
            self.registers[id]
                .snapshot(
                    cpu,
                    CpuIdentity {
                        domain: 2,
                        cpu: id as u32,
                    },
                    &mut output[CPUS_AT + id * CPU_BYTES..CPUS_AT + (id + 1) * CPU_BYTES],
                )
                .map_err(|_| InvalidState)?;
        }
        let end = STATE_BYTES - 32;
        let digest = Sha256::digest(&output[..end]);
        output[end..].copy_from_slice(&digest);
        Ok(())
    }

    pub unsafe fn restore_protected(
        &mut self,
        epoch: u64,
        input: &[u8],
    ) -> Result<(), InvalidState> {
        if !self.entry_approved {
            return Err(InvalidState);
        }
        unsafe {
            *self = Self::prepare_protected(epoch, input)?.install();
        }
        Ok(())
    }

    unsafe fn decode_record(
        &mut self,
        epoch: u64,
        input: &[u8],
    ) -> Result<[Vmcb; 4], InvalidState> {
        // Keep aligned scratch observable before early exits; see the pinned
        // compiler frame-setup workaround in secure_state::prepare_protected.
        let mut cpus = [const { Vmcb::new() }; 4];
        core::hint::black_box(&mut cpus);
        if input.len() != STATE_BYTES || epoch == 0 || !self.entry_approved {
            return Err(InvalidState);
        }
        let end = STATE_BYTES - 32;
        let word = |at| u64::from_le_bytes(input[at..at + 8].try_into().unwrap());
        if &input[..8] != b"BEXNW001"
            || input[8..12] != 2u32.to_le_bytes()
            || input[12..16] != 4u32.to_le_bytes()
            || word(16) != epoch
            || word(24) != BASE
            || word(32) != LENGTH as u64
            || input[40] >= 4
            || input[41] != 1
            || input[42] & !15 != 0
            || input[43..48] != [0; 5]
            || input[56..64] != [0; 8]
            || input[end..] != Sha256::digest(&input[..end])[..]
        {
            return Err(InvalidState);
        }
        #[cfg(feature = "secure_product")]
        if word(48) != self.evidence_address as u64 {
            return Err(InvalidState);
        }
        #[cfg(not(feature = "secure_product"))]
        if word(48) != 0 {
            return Err(InvalidState);
        }

        // Seed only trusted geometry. Import performs no BAR probing, DMA
        // remapping, queue reset or physical-device acknowledgement.
        let mut platform = Platform::for_restore(
            DomainMemory {
                base: BASE,
                length: LENGTH,
            },
            false,
        );
        platform.devices.pci = Some(unsafe { crate::pci::Pci::retained_policy()? });
        platform.restore_protected(
            CheckpointIdentity {
                domain: 2,
                clock_epoch: epoch,
            },
            unsafe { bexos_secure_monitor::clock::now_ns() },
            &input[HEADER..CPUS_AT],
        )?;
        let mut registers = core::array::from_fn(|_| Registers::default());
        for id in 0..4 {
            let running = platform.devices.fabric.state(id) == Some(CpuState::Running);
            if running != (input[42] & (1 << id) != 0) {
                return Err(InvalidState);
            }
            let bytes = &input[CPUS_AT + id * CPU_BYTES..CPUS_AT + (id + 1) * CPU_BYTES];
            if !running {
                if bytes.iter().any(|byte| *byte != 0) {
                    return Err(InvalidState);
                }
                continue;
            }
            registers[id]
                .restore_protected(
                    &mut cpus[id],
                    CpuIdentity {
                        domain: 2,
                        cpu: id as u32,
                    },
                    RestorePolicy {
                        asid: id as u32 + 2,
                        npt: self.root,
                        iopm: core::ptr::addr_of!(crate::IOPM) as u64,
                        msrpm: core::ptr::addr_of!(crate::MSRPM) as u64,
                        mxcsr_mask: LegacyExtendedState::supported_mxcsr_mask(),
                    },
                    bytes,
                )
                .map_err(|_| InvalidState)?;
        }
        // This object is private preparation, not the live normal-world owner.
        self.registers = registers;
        self.platform = platform;
        self.next = input[40] as usize;
        Ok(cpus)
    }

    #[cfg(any(
        feature = "normal_checkpoint_probe",
        feature = "monitor_transfer",
        feature = "nucleus_probe"
    ))]
    pub fn all_running_for_probe(&self) -> bool {
        (0..4).all(|id| self.platform.devices.fabric.state(id) == Some(CpuState::Running))
    }

    #[cfg(feature = "normal_checkpoint_probe")]
    pub unsafe fn assert_unchanged_for_probe(&self, record: &[u8]) {
        let epoch = u64::from_le_bytes(record[16..24].try_into().unwrap());
        let captured = u64::from_le_bytes(record[HEADER + 24..HEADER + 32].try_into().unwrap());
        let mut current = [0; STATE_BYTES];
        unsafe {
            self.snapshot_at(epoch, captured, &mut current).unwrap();
        }
        assert_eq!(current.as_slice(), record);
    }

    #[cfg(feature = "normal_checkpoint_probe")]
    pub unsafe fn checkpoint_probe(&mut self) {
        static mut DONE: bool = false;
        static mut RECORD: [u8; STATE_BYTES] = [0; STATE_BYTES];
        if unsafe { DONE }
            || (0..4).any(|id| self.platform.devices.fabric.state(id) != Some(CpuState::Running))
        {
            return;
        }
        unsafe {
            let record = &mut *core::ptr::addr_of_mut!(RECORD);
            self.snapshot(1, record).unwrap();
            let instruction_pointers: [u64; 4] =
                core::array::from_fn(|id| (*core::ptr::addr_of!(CPUS))[id].rip());
            // A corrupt last CPU must not install the first three CPUs or
            // advance platform state. Recompute the aggregate digest so the
            // nested CPU validator, rather than only the envelope, rejects it.
            let corrupted = CPUS_AT + 3 * CPU_BYTES + 24;
            record[corrupted] ^= 1;
            let end = STATE_BYTES - 32;
            let digest = Sha256::digest(&record[..end]);
            record[end..].copy_from_slice(&digest);
            assert!(self.restore_protected(1, record).is_err());
            for id in 0..4 {
                assert_eq!(
                    (*core::ptr::addr_of!(CPUS))[id].rip(),
                    instruction_pointers[id]
                );
            }
            record[corrupted] ^= 1;
            let digest = Sha256::digest(&record[..end]);
            record[end..].copy_from_slice(&digest);
            // Deliberately discard CPU objects to prove restoration is used.
            *core::ptr::addr_of_mut!(CPUS) = [const { Vmcb::new() }; 4];
            self.registers = core::array::from_fn(|_| Registers::default());
            self.platform = Platform::new(
                DomainMemory {
                    base: BASE,
                    length: LENGTH,
                },
                false,
            );
            self.root = 0;
            self.entry_approved = false;
            *self = Self::resume_protected(1, record).unwrap();
            DONE = true;
        }
        crate::log("monitor-runtime: four BexOS CPUs restored from protected aggregate\n");
    }
}
