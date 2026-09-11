//! Interrupt/timer handoff without replaying guest register writes or IPIs.
//! The resident owner supplies provenance and the unchanged monotonic clock
//! identity. Checksums detect damage; they do not authenticate a checkpoint.
use super::*;
use crate::{
    run_state::RunState,
    state_wire::{InvalidState, Reader, Result, State, Writer},
};
use sha2::{Digest, Sha256};

#[derive(Clone, Copy)]
pub struct CheckpointIdentity {
    pub domain: u32,
    pub clock_epoch: u64,
}
impl<const N: usize> Fabric<N> {
    pub const STATE_BYTES: usize =
        64 + N * (LocalApic::BYTES + 5) + Hpet::BYTES + IoApic::BYTES + LegacyIrq::BYTES + 32;

    /// All vCPUs must be stopped outside VMEXIT processing. Preserve the same
    /// clock through activation: timer/authentication lifetimes never restart.
    pub fn snapshot(
        &self,
        identity: CheckpointIdentity,
        now: u64,
        run: &[RunState; N],
        armed: &[Option<Delivery>; N],
        output: &mut [u8],
    ) -> Result<()> {
        if N == 0
            || N > 32
            || identity.domain == 0
            || identity.clock_epoch == 0
            || output.len() != Self::STATE_BYTES
        {
            return Err(InvalidState);
        }
        output.fill(0);
        let end = output.len() - 32;
        let mut w = Writer::new(&mut output[..end]);
        w.bytes(b"BEXIRQ01")?;
        w.u32(identity.domain)?;
        w.u32(N as u32)?;
        w.u64(now)?;
        w.u64(identity.clock_epoch)?;
        w.u64(Self::STATE_BYTES as u64)?;
        w.u32(u32::from(bexos_secure_monitor_abi::ARCH_X86_64))?;
        w.bytes(&[0; 20])?;
        for cpu in 0..N {
            self.apics[cpu].save(&mut w)?;
            let (phase, vector) = match self.states[cpu] {
                CpuState::Running => (0, 0),
                CpuState::WaitingForStartup => (1, 0),
                CpuState::Start(vector) => (2, vector),
            };
            w.u8(phase)?;
            w.u8(vector)?;
            w.boolean(run[cpu].halted())?;
            w.u8(armed[cpu].map_or(0, |value| if value.pic { 2 } else { 1 }))?;
            w.u8(armed[cpu].map_or(0, |value| value.vector))?;
        }
        self.hpet.save(&mut w)?;
        self.ioapic.save(&mut w)?;
        self.legacy.save(&mut w)?;
        w.finish()?;
        let digest = Sha256::digest(&output[..end]);
        output[end..].copy_from_slice(&digest);
        Ok(())
    }

    /// Decode to private temporaries before changing any live destination.
    /// Requires matching protected CPU records, including pending injection.
    pub fn restore_protected(
        &mut self,
        identity: CheckpointIdentity,
        now: u64,
        run: &mut [RunState; N],
        armed: &mut [Option<Delivery>; N],
        input: &[u8],
    ) -> Result<()> {
        if N == 0
            || N > 32
            || input.len() != Self::STATE_BYTES
            || identity.domain == 0
            || identity.clock_epoch == 0
        {
            return Err(InvalidState);
        }
        let end = input.len() - 32;
        if input[end..] != Sha256::digest(&input[..end])[..] {
            return Err(InvalidState);
        }
        let mut r = Reader::new(&input[..end]);
        if r.bytes::<8>()? != *b"BEXIRQ01" || r.u32()? != identity.domain || r.u32()? != N as u32 {
            return Err(InvalidState);
        }
        let captured = r.u64()?;
        if captured > now
            || r.u64()? != identity.clock_epoch
            || r.u64()? != Self::STATE_BYTES as u64
            || r.u32()? != u32::from(bexos_secure_monitor_abi::ARCH_X86_64)
            || r.bytes::<20>()? != [0; 20]
        {
            return Err(InvalidState);
        }
        let mut value = Self::new().ok_or(InvalidState)?;
        let mut halted = [RunState::default(); N];
        let mut deliveries = [None; N];
        for cpu in 0..N {
            value.apics[cpu] = LocalApic::load(&mut r, captured)?;
            if value.apics[cpu].identity() != cpu {
                return Err(InvalidState);
            }
            value.states[cpu] = match (r.u8()?, r.u8()?) {
                (0, 0) => CpuState::Running,
                (1, 0) => CpuState::WaitingForStartup,
                (2, vector) => CpuState::Start(vector),
                _ => return Err(InvalidState),
            };
            halted[cpu] = RunState::from_halted(r.boolean()?);
            deliveries[cpu] = match (r.u8()?, r.u8()?) {
                (0, 0) => None,
                (1, vector @ 16..=255) => Some(Delivery { vector, pic: false }),
                (2, vector) if cpu == 0 => Some(Delivery { vector, pic: true }),
                _ => return Err(InvalidState),
            };
            if deliveries[cpu].is_some() && value.states[cpu] != CpuState::Running {
                return Err(InvalidState);
            }
        }
        value.hpet = Hpet::load(&mut r, captured)?;
        value.ioapic = IoApic::load(&mut r, captured)?;
        if value.ioapic.cpu_count() != N {
            return Err(InvalidState);
        }
        value.legacy = LegacyIrq::load(&mut r, captured)?;
        r.finish()?;
        for (cpu, delivery) in deliveries.iter().enumerate() {
            if let Some(delivery) = delivery {
                let valid = if delivery.pic {
                    value.legacy.retains(delivery.vector)
                } else {
                    value.apics[cpu].retains(delivery.vector)
                };
                if !valid {
                    return Err(InvalidState);
                }
            }
        }
        *self = value;
        *run = halted;
        *armed = deliveries;
        Ok(())
    }
}
