//! Per-vCPU xAPIC state. All timing comes from the private monitor clock.
//! No guest register write is forwarded to the physical interrupt controller.
#[path = "lapic_state.rs"]
mod state;
const MASKED: u32 = 1 << 16;
const PERIODIC: u32 = 1 << 17;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IpiKind {
    Fixed(u8),
    Init,
    Startup(u8),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ipi {
    pub destination: u8,
    pub shorthand: u8,
    pub kind: IpiKind,
}
#[derive(Clone)]
pub struct LocalApic {
    id: u8,
    tpr: u8,
    svr: u32,
    ldr: u32,
    dfr: u32,
    lvt: [u32; 7],
    irr: [u32; 8],
    isr: [u32; 8],
    icr_high: u32,
    icr_low: u32,
    divide: u32,
    initial: u32,
    deadline_ns: Option<u64>,
}
impl LocalApic {
    pub const fn new(id: u8) -> Self {
        Self {
            id,
            tpr: 0,
            svr: 0xff,
            ldr: 0,
            dfr: u32::MAX,
            lvt: [MASKED; 7],
            irr: [0; 8],
            isr: [0; 8],
            icr_high: 0,
            icr_low: 0,
            divide: 0,
            initial: 0,
            deadline_ns: None,
        }
    }
    fn highest(bits: &[u32; 8]) -> Option<u8> {
        (0..8).rev().find_map(|i| {
            (bits[i] != 0).then(|| (i * 32 + 31 - bits[i].leading_zeros() as usize) as u8)
        })
    }
    fn priority(&self) -> u8 {
        (self.tpr & 0xf0).max(Self::highest(&self.isr).unwrap_or(0) & 0xf0)
    }
    pub fn raise(&mut self, vector: u8) -> bool {
        if vector < 16 {
            return false;
        }
        self.irr[vector as usize / 32] |= 1 << (vector % 32);
        true
    }
    pub fn pending(&self) -> Option<u8> {
        if self.svr & 0x100 == 0 {
            return None;
        }
        Self::highest(&self.irr).filter(|v| v & 0xf0 > self.priority())
    }
    /// Called only after hardware reports delivery of the armed virtual IRQ.
    pub fn delivered(&mut self, vector: u8) {
        self.irr[vector as usize / 32] &= !(1 << (vector % 32));
        self.isr[vector as usize / 32] |= 1 << (vector % 32);
    }
    fn divisor(&self) -> u64 {
        match self.divide & 0xb {
            0 => 2,
            1 => 4,
            2 => 8,
            3 => 16,
            8 => 32,
            9 => 64,
            10 => 128,
            _ => 1,
        }
    }
    fn interval(&self) -> u64 {
        u64::from(self.initial) * self.divisor() * 10
    } // 100 MHz input.
    fn current_count(&self, now: u64) -> u32 {
        let Some(deadline) = self.deadline_ns else {
            return 0;
        };
        let remaining = if now < deadline {
            deadline - now
        } else if self.lvt[0] & PERIODIC != 0 && self.initial != 0 {
            self.interval() - (now - deadline) % self.interval()
        } else {
            0
        };
        remaining
            .div_ceil(self.divisor() * 10)
            .min(u64::from(u32::MAX)) as u32
    }
    pub fn tick(&mut self, now: u64) {
        if !self.deadline_ns.is_some_and(|deadline| now >= deadline) {
            return;
        }
        if self.lvt[0] & MASKED == 0 {
            self.raise(self.lvt[0] as u8);
        }
        let interval = self.interval();
        self.deadline_ns = if self.lvt[0] & PERIODIC != 0 && interval != 0 {
            // Coalesce missed ticks; never loop for elapsed guest time.
            let deadline = self.deadline_ns.unwrap();
            let elapsed = now - deadline;
            deadline
                .checked_add(elapsed / interval * interval)
                .and_then(|base| base.checked_add(interval))
        } else {
            None
        };
    }
    pub fn read(&self, offset: u16, now: u64) -> Option<u32> {
        if offset & 15 != 0 {
            return None;
        }
        Some(match offset {
            0x20 => u32::from(self.id) << 24,
            0x30 => 0x00060014,
            0x80 => u32::from(self.tpr),
            0xa0 => u32::from(self.priority()),
            0xd0 => self.ldr,
            0xe0 => self.dfr,
            0xf0 => self.svr,
            0x100..=0x170 => self.isr[usize::from((offset - 0x100) / 16)],
            0x180..=0x1f0 => 0, // Edge-triggered sources only in this backend.
            0x200..=0x270 => self.irr[usize::from((offset - 0x200) / 16)],
            0x280 => 0,
            0x300 => self.icr_low,
            0x310 => self.icr_high,
            0x320..=0x370 => self.lvt[usize::from((offset - 0x320) / 16)],
            0x2f0 => self.lvt[6],
            0x380 => self.initial,
            0x390 => self.current_count(now),
            0x3e0 => self.divide,
            _ => return None,
        })
    }
    pub fn write(&mut self, offset: u16, value: u32, now: u64) -> Result<Option<Ipi>, ()> {
        match offset {
            0x80 if value <= 255 => self.tpr = value as u8,
            0xb0 if value == 0 => {
                if let Some(vector) = Self::highest(&self.isr) {
                    self.isr[vector as usize / 32] &= !(1 << (vector % 32));
                }
            }
            0xd0 if value & 0xffffff == 0 => self.ldr = value,
            0xe0 if value == u32::MAX => self.dfr = value,
            0xf0 if value & !0x3ff == 0 => self.svr = value,
            0x280 if value == 0 => {}
            0x310 if value & 0xffffff == 0 => self.icr_high = value,
            0x300 => {
                // Physical destinations only. Logical/broadcast routing must
                // be resolved against this domain's vCPUs by its scheduler.
                if value & !0xcc7ff != 0 || value & (1 << 11) != 0 {
                    return Err(());
                }
                let kind = match (value >> 8) & 7 {
                    0 if value as u8 >= 16 => IpiKind::Fixed(value as u8),
                    5 => IpiKind::Init,
                    6 => IpiKind::Startup(value as u8),
                    _ => return Err(()),
                };
                self.icr_low = value & !(1 << 12);
                // INIT deassert is an electrical compatibility operation.
                if kind == IpiKind::Init && value & (1 << 15) != 0 && value & (1 << 14) == 0 {
                    return Ok(None);
                }
                return Ok(Some(Ipi {
                    destination: (self.icr_high >> 24) as u8,
                    shorthand: ((value >> 18) & 3) as u8,
                    kind,
                }));
            }
            0x320 if value & !(PERIODIC | MASKED | 0xff) == 0 => self.lvt[0] = value,
            0x330..=0x370 if offset & 15 == 0 && value & MASKED != 0 => {
                self.lvt[usize::from((offset - 0x320) / 16)] = value
            }
            0x2f0 if value & MASKED != 0 => self.lvt[6] = value,
            0x380 => {
                self.initial = value;
                self.deadline_ns = (value != 0)
                    .then(|| now.checked_add(self.interval()))
                    .flatten();
            }
            0x3e0 if value & !0xb == 0 => {
                self.tick(now);
                let remaining = self.current_count(now);
                self.divide = value;
                self.deadline_ns = (remaining != 0)
                    .then(|| now.checked_add(u64::from(remaining) * self.divisor() * 10))
                    .flatten();
            }
            _ => return Err(()),
        }
        Ok(None)
    }
}
#[cfg(test)]
#[path = "lapic_tests.rs"]
mod tests;
