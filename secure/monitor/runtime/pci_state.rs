//! Software checkpoint against retained resident PCI geometry. This neither
//! probes BARs nor writes the physical device while queues are active.
use super::*;
use bexos_secure_monitor::{pci_state::Identity, state_wire::InvalidState};

#[unsafe(link_section = ".resident.pci_policy")]
static mut POLICY: [u8; 32 + Pci::STATE_BYTES] = [0; 32 + Pci::STATE_BYTES];

impl Pci {
    pub const STATE_BYTES: usize = Function::STATE_BYTES * REQUESTERS.len();
    fn identity(index: usize) -> Identity {
        Identity {
            requester: REQUESTERS[index],
            vendor_device: IDENTITIES[index],
        }
    }
    /// Cold boot captures geometry before any guest configures the devices.
    /// A replacement reads this region instead of probing live BARs or
    /// borrowing an object inside the retiring monitor image.
    pub unsafe fn retain_policy(&self) {
        let bytes = unsafe { &mut *core::ptr::addr_of_mut!(POLICY) };
        assert_eq!(&bytes[..32], &[0; 32]);
        self.snapshot(&mut bytes[32..]).unwrap();
        bytes[8..16].copy_from_slice(&2u64.to_le_bytes());
        bytes[16..24].copy_from_slice(&(REQUESTERS.len() as u64).to_le_bytes());
        bytes[..8].copy_from_slice(b"BEXPG001");
    }
    pub unsafe fn retained_policy() -> Result<Self, InvalidState> {
        let bytes = unsafe { &*core::ptr::addr_of!(POLICY) };
        if &bytes[..8] != b"BEXPG001"
            || bytes[8..16] != 2u64.to_le_bytes()
            || bytes[16..24] != (REQUESTERS.len() as u64).to_le_bytes()
            || bytes[24..32] != [0; 8]
        {
            return Err(InvalidState);
        }
        let mut result = Self {
            functions: [Function::default(); REQUESTERS.len()],
        };
        for (index, input) in bytes[32..].chunks_exact(Function::STATE_BYTES).enumerate() {
            result.functions[index] =
                Function::from_protected_policy(Self::identity(index), input)?;
        }
        Ok(result)
    }
    pub fn snapshot(&self, output: &mut [u8]) -> Result<(), InvalidState> {
        if output.len() != Self::STATE_BYTES {
            return Err(InvalidState);
        }
        for (index, bytes) in output.chunks_exact_mut(Function::STATE_BYTES).enumerate() {
            self.functions[index].snapshot(Self::identity(index), bytes)?;
        }
        Ok(())
    }
    pub fn restore_protected(&self, input: &[u8]) -> Result<Self, InvalidState> {
        if input.len() != Self::STATE_BYTES {
            return Err(InvalidState);
        }
        let mut restored = self.clone();
        for (index, bytes) in input.chunks_exact(Function::STATE_BYTES).enumerate() {
            restored.functions[index] =
                Function::restore_protected(Self::identity(index), &self.functions[index], bytes)?;
        }
        if !Function::disjoint(&restored.functions) {
            return Err(InvalidState);
        }
        Ok(restored)
    }
}
