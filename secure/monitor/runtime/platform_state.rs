//! Complete virtual-platform records, decoded before any live state changes.
//! Physical DMA tables and CPU/shared-memory records are separate resident
//! responsibilities; importing this record does not activate a candidate.
use super::*;
use bexos_secure_monitor::{
    cmos::Clock,
    fabric::{CheckpointIdentity, Fabric},
    run_state::RunState,
    state_wire::InvalidState,
    uart::Uart,
};
use sha2::{Digest, Sha256};

impl<const N: usize> Platform<N> {
    pub const STATE_BYTES: usize =
        64 + Fabric::<N>::STATE_BYTES + Uart::STATE_BYTES + 160 + crate::pci::Pci::STATE_BYTES + 32;
    pub fn snapshot(
        &self,
        identity: CheckpointIdentity,
        now: u64,
        output: &mut [u8],
    ) -> Result<(), InvalidState> {
        if output.len() != Self::STATE_BYTES || identity.domain != if self.rpmb { 1 } else { 2 } {
            return Err(InvalidState);
        }
        output.fill(0);
        output[..8].copy_from_slice(b"BEXPL001");
        output[8..12].copy_from_slice(&identity.domain.to_le_bytes());
        output[12..16].copy_from_slice(&(N as u32).to_le_bytes());
        for (offset, value) in [
            (16, identity.clock_epoch),
            (24, now),
            (32, self.memory.base),
            (40, self.memory.length as u64),
        ] {
            output[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
        }
        output[48] = u8::from(self.rpmb);
        output[49] = u8::from(self.rtc.is_some());
        output[50] = u8::from(self.devices.pci.is_some());
        output[51] = u8::from(self.devices.delivered);
        let mut offset = 64;
        self.devices.fabric.snapshot(
            identity,
            now,
            &self.run,
            &self.devices.armed,
            &mut output[offset..offset + Fabric::<N>::STATE_BYTES],
        )?;
        offset += Fabric::<N>::STATE_BYTES;
        self.console
            .snapshot(&mut output[offset..offset + Uart::STATE_BYTES])?;
        offset += Uart::STATE_BYTES;
        if let Some(rtc) = &self.rtc {
            rtc.snapshot(&mut output[offset..offset + Clock::STATE_BYTES])?;
        }
        offset += 160;
        if let Some(pci) = &self.devices.pci {
            pci.snapshot(&mut output[offset..offset + crate::pci::Pci::STATE_BYTES])?;
        }
        offset += crate::pci::Pci::STATE_BYTES;
        let digest = Sha256::digest(&output[..offset]);
        output[offset..].copy_from_slice(&digest);
        Ok(())
    }
    /// The retained owner supplies memory/device policy and clock identity.
    /// Imported records contain no host pointers or physical-device commands.
    pub fn restore_protected(
        &mut self,
        identity: CheckpointIdentity,
        now: u64,
        input: &[u8],
    ) -> Result<(), InvalidState> {
        if input.len() != Self::STATE_BYTES || identity.domain != if self.rpmb { 1 } else { 2 } {
            return Err(InvalidState);
        }
        let end = input.len() - 32;
        let word = |offset| u64::from_le_bytes(input[offset..offset + 8].try_into().unwrap());
        if &input[..8] != b"BEXPL001"
            || input[8..12] != identity.domain.to_le_bytes()
            || input[12..16] != (N as u32).to_le_bytes()
            || word(16) != identity.clock_epoch
            || word(24) > now
            || word(32) != self.memory.base
            || word(40) != self.memory.length as u64
            || input[48] != u8::from(self.rpmb)
            || input[49] != u8::from(!self.rpmb)
            || input[50] != u8::from(self.devices.pci.is_some())
            || input[51] > 1
            || input[52..64] != [0; 12]
            || input[end..] != Sha256::digest(&input[..end])[..]
        {
            return Err(InvalidState);
        }
        let captured = word(24);
        let mut fabric = Fabric::<N>::new().ok_or(InvalidState)?;
        let mut run = [RunState::default(); N];
        let mut armed = [None; N];
        let mut offset = 64;
        let fabric_bytes = &input[offset..offset + Fabric::<N>::STATE_BYTES];
        if u64::from_le_bytes(fabric_bytes[16..24].try_into().unwrap()) != captured {
            return Err(InvalidState);
        }
        fabric.restore_protected(identity, now, &mut run, &mut armed, fabric_bytes)?;
        offset += Fabric::<N>::STATE_BYTES;
        let mut console =
            crate::console::Console::new(if self.rpmb { "[trusty] " } else { "[bexos] " });
        console.restore_protected(&input[offset..offset + Uart::STATE_BYTES])?;
        offset += Uart::STATE_BYTES;
        let rtc = if !self.rpmb {
            if input[offset + Clock::STATE_BYTES..offset + 160]
                .iter()
                .any(|byte| *byte != 0)
            {
                return Err(InvalidState);
            }
            Some(Clock::restore_protected(
                &input[offset..offset + Clock::STATE_BYTES],
                captured,
            )?)
        } else {
            if input[offset..offset + 160].iter().any(|byte| *byte != 0) {
                return Err(InvalidState);
            }
            None
        };
        offset += 160;
        let pci = if let Some(policy) = &self.devices.pci {
            Some(policy.restore_protected(&input[offset..end])?)
        } else {
            if input[offset..end].iter().any(|byte| *byte != 0) {
                return Err(InvalidState);
            }
            None
        };
        self.devices.fabric = fabric;
        self.devices.armed = armed;
        self.devices.pci = pci;
        self.devices.delivered = input[51] != 0;
        self.run = run;
        self.console = console;
        self.rtc = rtc;
        Ok(())
    }
}
