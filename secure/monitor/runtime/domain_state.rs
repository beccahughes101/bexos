//! One protected record for both execution domains and their shared transport.
//! Validation of the second domain cannot partially install the first domain.
use crate::{normal::Normal, platform::Platform};
use bexos_secure_monitor::{state_wire::InvalidState, svm::Vmcb, vcpu::Registers};
use sha2::{Digest, Sha256};

const HEADER: usize = 64;
const SECURE_AT: usize = HEADER + crate::normal::STATE_BYTES;
const END: usize = SECURE_AT + crate::secure_state::STATE_BYTES;
pub const STATE_BYTES: usize = END + 32;

pub struct Prepared {
    normal: crate::normal::Prepared,
    secure: crate::secure_state::Prepared,
}
impl Prepared {
    /// Reconnect the shared transport after both domain records validate.
    pub unsafe fn install_transport_only(self) {
        unsafe {
            self.secure.install_transport_only();
        }
        core::hint::black_box(self.normal);
    }

    /// Construct both software owners after entry into a different image.
    /// This path does not borrow either retiring software owner.
    pub unsafe fn install_owners(self, vmcb: &mut Vmcb) -> (Normal, Registers, Platform<1>) {
        let mut registers = Registers::default();
        let mut platform = Platform::<1>::new(
            crate::memory::DomainMemory {
                base: crate::BANK,
                length: crate::BANK_SIZE,
            },
            true,
        );
        unsafe {
            self.secure
                .install_preserving_cpu(&mut registers, &mut platform);
            core::hint::black_box(vmcb);
            (self.normal.install_preserving_cpus(), registers, platform)
        }
    }
    /// Both domains remain stopped until this infallible installation finishes.
    pub unsafe fn install(
        self,
        normal: &mut Normal,
        vmcb: &mut Vmcb,
        registers: &mut Registers,
        platform: &mut Platform<1>,
    ) {
        unsafe {
            self.secure.install_preserving_cpu(registers, platform);
            core::hint::black_box(vmcb);
            *normal = self.normal.install_preserving_cpus();
        }
    }
}

/// Keep both domains stopped. The output must be exclusively protected memory.
pub unsafe fn snapshot(
    epoch: u64,
    normal: &Normal,
    vmcb: &Vmcb,
    registers: &Registers,
    platform: &Platform<1>,
    output: &mut [u8],
) -> Result<(), InvalidState> {
    if epoch == 0 || output.len() != STATE_BYTES {
        return Err(InvalidState);
    }
    output.fill(0);
    output[..8].copy_from_slice(b"BEXDW001");
    output[8..12].copy_from_slice(&2u32.to_le_bytes());
    output[12..16].copy_from_slice(&2u32.to_le_bytes());
    output[16..24].copy_from_slice(&epoch.to_le_bytes());
    output[24..32].copy_from_slice(&(crate::normal::STATE_BYTES as u64).to_le_bytes());
    output[32..40].copy_from_slice(&(crate::secure_state::STATE_BYTES as u64).to_le_bytes());
    unsafe {
        normal.snapshot(epoch, &mut output[HEADER..SECURE_AT])?;
        crate::secure_state::snapshot(
            epoch,
            vmcb,
            registers,
            platform,
            &mut output[SECURE_AT..END],
        )?;
    }
    let digest = Sha256::digest(&output[..END]);
    output[END..].copy_from_slice(&digest);
    Ok(())
}

/// Only the protected owner may supply these bytes. A checksum does not grant
/// authority. Dropping the result changes neither execution domain nor transport.
pub unsafe fn prepare_protected(epoch: u64, input: &[u8]) -> Result<Prepared, InvalidState> {
    // Materialize the required page alignment before any early-return edge.
    // The pinned compiler otherwise shrink-wraps away this function's RBP
    // setup while retaining an RBP-based epilogue (observed in the actual
    // two-image x86 guest). Keep this scratch uninitialized; it is never read.
    let mut alignment = core::mem::MaybeUninit::<Vmcb>::uninit();
    core::hint::black_box(&mut alignment);
    if epoch == 0 || input.len() != STATE_BYTES {
        return Err(InvalidState);
    }
    if &input[..8] != b"BEXDW001"
        || input[8..12] != 2u32.to_le_bytes()
        || input[12..16] != 2u32.to_le_bytes()
        || input[16..24] != epoch.to_le_bytes()
        || input[24..32] != (crate::normal::STATE_BYTES as u64).to_le_bytes()
        || input[32..40] != (crate::secure_state::STATE_BYTES as u64).to_le_bytes()
        || input[40..HEADER] != [0; HEADER - 40]
        || input[END..] != Sha256::digest(&input[..END])[..]
    {
        return Err(InvalidState);
    }
    unsafe {
        let normal = Normal::prepare_protected(epoch, &input[HEADER..SECURE_AT])?;
        let secure = crate::secure_state::prepare_protected(epoch, &input[SECURE_AT..END])?;
        Ok(Prepared { normal, secure })
    }
}

#[cfg(feature = "normal_checkpoint_probe")]
pub unsafe fn probe(
    normal: &mut Normal,
    vmcb: &mut Vmcb,
    registers: &mut Registers,
    platform: &mut Platform<1>,
) {
    static mut DONE: bool = false;
    static mut RECORD: [u8; STATE_BYTES] = [0; STATE_BYTES];
    if unsafe { DONE } || !normal.all_running_for_probe() {
        return;
    }
    unsafe {
        let record = &mut *core::ptr::addr_of_mut!(RECORD);
        snapshot(1, normal, vmcb, registers, platform, record).unwrap();
        crate::secure_state::corrupt_transport_for_probe(&mut record[SECURE_AT..END]);
        let digest = Sha256::digest(&record[..END]);
        record[END..].copy_from_slice(&digest);
        assert!(prepare_protected(1, record).is_err());
        crate::secure_state::corrupt_transport_for_probe(&mut record[SECURE_AT..END]);
        let digest = Sha256::digest(&record[..END]);
        record[END..].copy_from_slice(&digest);
        normal.assert_unchanged_for_probe(&record[HEADER..SECURE_AT]);
        crate::secure_state::assert_unchanged_for_probe(
            &record[SECURE_AT..END],
            vmcb,
            registers,
            platform,
        );
        // Validate a complete independently reconstructed owner. The resident
        // execution owner keeps the live VMCBs, registers and device state.
        core::hint::black_box(prepare_protected(1, record).unwrap());
        core::hint::black_box((&mut *normal, &mut *vmcb, &mut *registers, &mut *platform));
        DONE = true;
    }
    crate::log("monitor-runtime: both domain records validated after atomic handoff rejection\n");
}
