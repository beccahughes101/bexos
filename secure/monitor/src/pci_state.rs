//! Assigned-device software state. The resident owner must separately quiesce
//! DMA, validate physical configuration, and install the replacement IOMMU
//! roots. Restoring this record performs no physical PCI or MMIO accesses.
use crate::{
    pci_policy::Bar,
    state_wire::{InvalidState, Reader, Writer},
    virtio_policy::Features,
};

#[derive(Clone, Copy, Default)]
pub struct Function {
    pub bars: [Bar; 6],
    pub command: u16,
    pub common_bar: Option<u8>,
    pub common_offset: u64,
    pub features: Features,
}
#[derive(Clone, Copy)]
pub struct Identity {
    pub requester: u8,
    pub vendor_device: u32,
}
impl Function {
    pub const STATE_BYTES: usize = 192;
    /// Reconstruct geometry captured by the boot owner before enabling DMA.
    /// This accepts only a pristine policy record from protected resident
    /// memory, never a guest checkpoint or physical device configuration.
    pub fn from_protected_policy(identity: Identity, input: &[u8]) -> Result<Self, InvalidState> {
        if input.len() != Self::STATE_BYTES {
            return Err(InvalidState);
        }
        let mut policy = Self::default();
        for (index, bar) in policy.bars.iter_mut().enumerate() {
            let offset = 16 + index * 24;
            let length = u64::from_le_bytes(input[offset + 8..offset + 16].try_into().unwrap());
            let flags = input[offset + 16];
            if length != 0 {
                *bar = Bar::new(length, flags).ok_or(InvalidState)?;
            } else if flags != 0 {
                return Err(InvalidState);
            }
        }
        let common = u32::from_le_bytes(input[164..168].try_into().unwrap());
        policy.common_bar = if common == u32::MAX {
            None
        } else {
            Some(u8::try_from(common).map_err(|_| InvalidState)?)
        };
        policy.common_offset = u64::from_le_bytes(input[168..176].try_into().unwrap());
        let restored = Self::restore_protected(identity, &policy, input)?;
        if restored.command != 0
            || restored.features.snapshot() != [0; 8]
            || restored
                .bars
                .iter()
                .any(|bar| bar.base != 0 || bar.probe != 0)
        {
            return Err(InvalidState);
        }
        Ok(restored)
    }
    pub fn snapshot(&self, identity: Identity, output: &mut [u8]) -> Result<(), InvalidState> {
        if output.len() != Self::STATE_BYTES {
            return Err(InvalidState);
        }
        let mut w = Writer::new(output);
        w.bytes(b"BEXPCI01")?;
        w.u8(identity.requester)?;
        w.u8(bexos_secure_monitor_abi::ARCH_X86_64 as u8)?;
        w.bytes(&[0; 2])?;
        w.u32(identity.vendor_device)?;
        for bar in self.bars {
            w.u64(bar.base)?;
            w.u64(bar.length)?;
            w.u8(bar.flags)?;
            w.u8(bar.probe)?;
            w.bytes(&[0; 6])?;
        }
        w.u32(u32::from(self.command))?;
        w.u32(self.common_bar.map_or(u32::MAX, u32::from))?;
        w.u64(self.common_offset)?;
        w.bytes(&self.features.snapshot())?;
        w.bytes(&[0; 8])?;
        w.finish()
    }
    /// `policy` contains resident-validated device geometry. Imported records
    /// cannot select another requester, invent BARs, or move common registers.
    /// The outer checkpoint must bind the domain and authenticate provenance.
    pub fn restore_protected(
        identity: Identity,
        policy: &Self,
        input: &[u8],
    ) -> Result<Self, InvalidState> {
        if input.len() != Self::STATE_BYTES {
            return Err(InvalidState);
        }
        let mut r = Reader::new(input);
        if r.bytes::<8>()? != *b"BEXPCI01"
            || r.u8()? != identity.requester
            || r.u8()? != bexos_secure_monitor_abi::ARCH_X86_64 as u8
            || r.bytes::<2>()? != [0; 2]
            || r.u32()? != identity.vendor_device
        {
            return Err(InvalidState);
        }
        let mut value = Self::default();
        for index in 0..6 {
            let base = r.u64()?;
            let length = r.u64()?;
            let flags = r.u8()?;
            let probe = r.u8()?;
            if r.bytes::<6>()? != [0; 6]
                || length != policy.bars[index].length
                || flags != policy.bars[index].flags
                || flags > 15
            {
                return Err(InvalidState);
            }
            if length == 0 {
                if base != 0 || flags != 0 || probe != 0 {
                    return Err(InvalidState);
                }
                continue;
            }
            let mut bar = Bar::new(length, flags).ok_or(InvalidState)?;
            if base > u64::from(u32::MAX)
                || probe & !(if bar.wide() { 3 } else { 1 }) != 0
                || index > 0 && value.bars[index - 1].wide()
                || index == 5 && bar.wide()
            {
                return Err(InvalidState);
            }
            bar = bar.changed(false, base as u32).ok_or(InvalidState)?;
            if bar.base != base || value.bars[..index].iter().any(|other| bar.overlaps(*other)) {
                return Err(InvalidState);
            }
            bar.probe = probe;
            value.bars[index] = bar;
        }
        value.command = u16::try_from(r.u32()?).map_err(|_| InvalidState)?;
        let common = r.u32()?;
        value.common_bar = if common == u32::MAX {
            None
        } else {
            Some(u8::try_from(common).map_err(|_| InvalidState)?)
        };
        value.common_offset = r.u64()?;
        value.features = Features::restore(r.bytes()?).ok_or(InvalidState)?;
        if r.bytes::<8>()? != [0; 8]
            || value.command & !0x406 != 0
            || value.command & 6 != 0
                && value
                    .bars
                    .iter()
                    .any(|bar| bar.length != 0 && bar.base == 0)
            || value.common_bar != policy.common_bar
            || value.common_offset != policy.common_offset
        {
            return Err(InvalidState);
        }
        if let Some(common) = value.common_bar {
            let bar = value.bars.get(usize::from(common)).ok_or(InvalidState)?;
            if bar.length == 0
                || !value
                    .common_offset
                    .checked_add(56)
                    .is_some_and(|end| end <= bar.length)
            {
                return Err(InvalidState);
            }
        } else if value.common_offset != 0 || value.features.snapshot() != [0; 8] {
            return Err(InvalidState);
        }
        r.finish()?;
        Ok(value)
    }
    /// Validate the complete domain inventory before installing any BARs.
    pub fn disjoint(functions: &[Self]) -> bool {
        for (index, function) in functions.iter().enumerate() {
            for other in &functions[..index] {
                if function
                    .bars
                    .iter()
                    .any(|bar| other.bars.iter().any(|other| bar.overlaps(*other)))
                {
                    return false;
                }
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const ID: Identity = Identity {
        requester: 32,
        vendor_device: 0x10431af4,
    };
    #[test]
    fn resident_policy_cannot_adopt_an_active_device_record() {
        let mut policy = Function::default();
        policy.bars[0] = Bar::new(0x4000, 4).unwrap();
        policy.common_bar = Some(0);
        let mut bytes = [0; Function::STATE_BYTES];
        policy.snapshot(ID, &mut bytes).unwrap();
        let retained = Function::from_protected_policy(ID, &bytes).unwrap();
        assert_eq!(retained.bars[0].length, 0x4000);
        assert_eq!(retained.common_bar, Some(0));
        for index in [8, 9, 16, 24, 32, 33, 34, 160, 164, 171, 176, 184] {
            let mut bad = bytes;
            bad[index] ^= 0x80;
            assert!(
                Function::from_protected_policy(ID, &bad).is_err(),
                "offset {index}"
            );
        }
        policy.bars[0] = policy.bars[0].changed(false, 0xc0000000).unwrap();
        policy.snapshot(ID, &mut bytes).unwrap();
        assert!(Function::from_protected_policy(ID, &bytes).is_err());
    }
    #[test]
    fn retains_probe_and_platform_negotiation_but_cannot_import_new_device_authority() {
        let mut policy = Function::default();
        policy.bars[0] = Bar::new(0x4000, 4).unwrap();
        policy.common_bar = Some(0);
        let mut source = policy;
        source.bars[0] = source.bars[0]
            .changed(false, 0xc0000000)
            .unwrap()
            .changed(true, u32::MAX)
            .unwrap();
        source.command = 0x406;
        source.features.write(8, 4, 1);
        source.features.write(12, 4, 3);
        let mut bytes = [0; Function::STATE_BYTES];
        source.snapshot(ID, &mut bytes).unwrap();
        let target = Function::restore_protected(ID, &policy, &bytes).unwrap();
        assert_eq!(target.bars[0].read(true), u32::MAX);
        assert!(target.features.ready());
        assert_eq!(target.command, 0x406);
        assert!(!Function::disjoint(&[source, target]));
        assert!(
            Function::restore_protected(
                Identity {
                    requester: 40,
                    ..ID
                },
                &policy,
                &bytes
            )
            .is_err()
        );
        for (offset, value) in [
            (16, 1),
            (24, 1),
            (33, 4),
            (34, 1),
            (160, 1),
            (164, 1),
            (176, 2),
            (180, 2),
            (184, 1),
        ] {
            let mut damaged = bytes;
            damaged[offset] = value;
            assert!(
                Function::restore_protected(ID, &policy, &damaged).is_err(),
                "offset {offset}"
            );
        }
    }
}
