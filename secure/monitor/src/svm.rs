//! AMD SVM control blocks. Byte offsets follow the architectural VMCB format;
//! no Rust object layout crosses the CPU interface.

pub const EFER: u32 = 0xc000_0080;
pub const PAT: u32 = 0x277;
pub const VM_HSAVE_PA: u32 = 0xc001_0117;
pub const SVME: u64 = 1 << 12;
pub const EXIT_IOIO: u64 = 0x7b;
pub const EXIT_MSR: u64 = 0x7c;
pub const EXIT_VMRUN: u64 = 0x80;
pub const EXIT_VMMCALL: u64 = 0x81;
pub const EXIT_NPF: u64 = 0x400;
pub const EXIT_CPUID: u64 = 0x72;
pub const EXIT_NMI: u64 = 0x61;
pub const EXIT_HLT: u64 = 0x78;
pub const EXIT_INVALID: u64 = u64::MAX;

#[repr(C, align(4096))]
pub struct Vmcb([u8; 4096]);
#[path = "svm_state.rs"]
mod state;
pub use state::RestorePolicy;

impl Default for Vmcb {
    fn default() -> Self {
        Self::new()
    }
}

impl Vmcb {
    pub const fn new() -> Self {
        Self([0; 4096])
    }

    fn write16(&mut self, offset: usize, value: u16) {
        self.0[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }
    fn write32(&mut self, offset: usize, value: u32) {
        self.0[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    fn write64(&mut self, offset: usize, value: u64) {
        self.0[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }
    fn read64(&self, offset: usize) -> u64 {
        u64::from_le_bytes(self.0[offset..offset + 8].try_into().unwrap())
    }

    fn segment(&mut self, offset: usize, selector: u16, attributes: u16) {
        self.write16(offset, selector);
        self.write16(offset + 2, attributes);
        self.write32(offset + 4, u32::MAX);
    }

    /// Initialize an isolated long-mode vCPU. The caller owns the backing pages,
    /// installs its guest page tables, and supplies their guest physical address.
    pub fn initialize(
        &mut self,
        asid: u32,
        npt: u64,
        cr3: u64,
        entry: u64,
        stack: u64,
    ) -> Result<(), InvalidVmcb> {
        if asid == 0
            || npt == 0
            || npt & 4095 != 0
            || npt >= 1 << 52
            || cr3 & 4095 != 0
            || cr3 >= 1 << 52
            || !canonical(entry)
            || !canonical(stack)
        {
            return Err(InvalidVmcb);
        }
        *self = Self::new();
        // Software entry currently switches x87/SSE state. Do not allow a
        // guest to enable XSAVE or modify monitor-owned debug registers.
        self.write16(0x02, 1 << 4); // CR4 writes.
        self.write16(0x04, 0xffff); // DR reads.
        self.write16(0x06, 0xffff); // DR writes.
        // Intercept HLT, I/O, MSRs, shutdown, nested VMRUN, and VMMCALL.
        self.write32(
            0x0c,
            (1 << 15) | (1 << 22) | (1 << 24) | (1 << 27) | (1 << 28) | (1 << 31),
        );
        // Cache invalidation, performance counters, MONITOR/MWAIT and XSETBV
        // affect state the monitor owns or does not context-switch.
        self.write32(
            0x10,
            0x7f | (1 << 9) | (1 << 10) | (1 << 11) | (1 << 13) | (1 << 14),
        );
        self.write32(0x58, asid);
        self.0[0x5c] = 1; // Flush all TLB entries on first entry.
        self.write64(0x90, 1); // Nested paging enabled.
        self.write64(0xb0, npt);
        for offset in [0x400, 0x420, 0x430, 0x440, 0x450] {
            self.segment(offset, 16, 0xc93);
        }
        self.segment(0x410, 8, 0xa9b);
        self.segment(0x490, 24, 0x8b); // Busy 64-bit TSS.
        self.write64(0x4d0, SVME | 0xd00); // LME, LMA, NXE.
        self.write64(0x548, 0x620); // PAE, OSFXSR, OSXMMEXCPT.
        self.write64(0x550, cr3);
        self.write64(0x558, 0x8001_0033); // PE, MP, ET, NE, WP, PG.
        self.write64(0x560, 0x400);
        self.write64(0x568, 0xffff_0ff0);
        self.write64(0x570, 2);
        self.write64(0x668, 0x0007_0406_0007_0406); // Architectural PAT reset value.
        self.set_rip(entry);
        self.write64(0x5d8, stack);
        Ok(())
    }
    pub fn set_permission_maps(&mut self, iopm: u64, msrpm: u64) -> Result<(), InvalidVmcb> {
        if iopm == 0
            || msrpm == 0
            || (iopm | msrpm) & 4095 != 0
            || iopm > (1 << 52) - 12288
            || msrpm > (1 << 52) - 8192
        {
            return Err(InvalidVmcb);
        }
        self.write64(0x40, iopm);
        self.write64(0x48, msrpm);
        Ok(())
    }
    pub fn set_rip(&mut self, value: u64) {
        self.write64(0x578, value);
    }
    pub fn intercept_cpuid(&mut self) {
        let flags = u32::from_le_bytes(self.0[0x0c..0x10].try_into().unwrap());
        self.write32(0x0c, flags | (1 << 18));
    }
    pub fn intercept_nmi(&mut self) {
        let flags = u32::from_le_bytes(self.0[0x0c..0x10].try_into().unwrap());
        self.write32(0x0c, flags | (1 << 1));
    }
    pub fn cr4(&self) -> u64 {
        self.read64(0x548)
    }
    pub fn cpl(&self) -> u8 {
        self.0[0x4cb]
    }
    pub fn set_cr4(&mut self, value: u64) {
        self.write64(0x548, value);
    }
    pub fn efer(&self) -> u64 {
        self.read64(0x4d0)
    }
    pub fn set_efer(&mut self, value: u64) {
        self.write64(0x4d0, value);
    }
    pub fn next_rip(&self) -> u64 {
        self.read64(0xc8)
    }
    pub fn cr3(&self) -> u64 {
        self.read64(0x550)
    }
    pub fn complete_halt(&mut self) {
        self.set_rip(self.rip() + 1);
        // HLT consumes the instruction following STI/MOV SS. Do not retain
        // that interrupt shadow when the scheduler wakes the stopped vCPU.
        self.write64(0x68, self.read64(0x68) & !1);
    }
    pub fn rflags(&self) -> u64 {
        self.read64(0x570)
    }
    pub fn set_rflags(&mut self, value: u64) {
        self.write64(0x570, value);
    }
    pub fn cr0(&self) -> u64 {
        self.read64(0x558)
    }
    pub fn code_base(&self) -> u64 {
        self.read64(0x418)
    }
    pub fn long_code(&self) -> bool {
        self.0[0x413] & 2 != 0
    }
    /// Start a Multiboot-compatible flat protected-mode guest. Guest paging
    /// starts disabled; NPT remains enabled and enforces the assigned bank.
    pub fn initialize_protected(
        &mut self,
        asid: u32,
        npt: u64,
        entry: u32,
    ) -> Result<(), InvalidVmcb> {
        self.initialize(asid, npt, 0, u64::from(entry), 0)?;
        self.segment(0x410, 8, 0xc9b);
        self.write64(0x4d0, SVME);
        self.write64(0x548, 0);
        self.write64(0x558, 0x33);
        Ok(())
    }
    /// A SIPI selects real-mode CS:IP within the guest's own low RAM. The
    /// scheduler must accept this only for a waiting vCPU in the same domain.
    pub fn initialize_startup(
        &mut self,
        asid: u32,
        npt: u64,
        vector: u8,
    ) -> Result<(), InvalidVmcb> {
        self.initialize_protected(asid, npt, 0)?;
        for offset in [0x400, 0x420, 0x430, 0x440, 0x450] {
            self.segment(offset, 0, 0x93);
            self.write32(offset + 4, 0xffff);
        }
        self.segment(0x410, u16::from(vector) << 8, 0x9b);
        self.write32(0x414, 0xffff);
        self.write64(0x418, u64::from(vector) << 12);
        self.write64(0x558, 0x10);
        Ok(())
    }
    pub fn rsp(&self) -> u64 {
        self.read64(0x5d8)
    }
    pub fn set_rsp(&mut self, value: u64) {
        self.write64(0x5d8, value);
    }
    pub fn set_idtr(&mut self, base: u64, limit: u16) -> Result<(), InvalidVmcb> {
        if !canonical(base) {
            return Err(InvalidVmcb);
        }
        self.write64(0x488, base);
        self.write32(0x484, u32::from(limit));
        Ok(())
    }
    pub fn set_gdtr(&mut self, base: u64, limit: u16) -> Result<(), InvalidVmcb> {
        if !canonical(base) {
            return Err(InvalidVmcb);
        }
        self.write64(0x468, base);
        self.write32(0x464, u32::from(limit));
        Ok(())
    }
    /// Keep physical maskable interrupts owned by the monitor. A virtual IRQ
    /// is delivered by VMRUN only when guest IF and interrupt shadow permit it.
    pub fn virtual_interrupt(&mut self, vector: Option<u8>) {
        self.write32(
            0x60,
            (1 << 24) | vector.map_or(0, |v| (1 << 8) | (u32::from(v >> 4) << 16)),
        );
        self.write32(0x64, u32::from(vector.unwrap_or(0)));
    }
    pub fn virtual_interrupt_pending(&self) -> bool {
        self.0[0x61] & 1 != 0
    }
    /// Access only MSRs in the architectural VMCB save area. VMLOAD/VMSAVE
    /// handle the syscall MSRs; VMRUN/VMEXIT switch guest PAT with nested paging.
    /// This never reads or writes host CPU registers.
    pub fn saved_msr(&self, msr: u32) -> Option<u64> {
        Some(self.read64(saved_msr_offset(msr)?))
    }
    pub fn set_saved_msr(&mut self, msr: u32, value: u64) -> bool {
        // VMLOAD executes in the monitor before VMRUN performs consistency
        // checks. A guest must never feed it a noncanonical address or reserved
        // field bits and turn a guest WRMSR into a host exception.
        let valid = match msr {
            PAT => value
                .to_le_bytes()
                .iter()
                .all(|entry| matches!(entry, 0 | 1 | 4..=7)),
            0xc0000081 => value as u32 == 0, // STAR's low 32 bits are reserved.
            0xc0000084 | 0x174 => value <= u32::MAX as u64,
            0xc0000082 | 0xc0000083 | 0xc0000100..=0xc0000102 | 0x175 | 0x176 => canonical(value),
            _ => false,
        };
        if valid {
            if let Some(offset) = saved_msr_offset(msr) {
                self.write64(offset, value);
                if msr == PAT {
                    self.0[0x5c] = 1; // Drop translations retaining the previous memory type.
                }
                return true;
            }
        }
        false
    }
    pub fn rip(&self) -> u64 {
        self.read64(0x578)
    }
    pub fn rax(&self) -> u64 {
        self.read64(0x5f8)
    }
    pub fn set_rax(&mut self, value: u64) {
        self.write64(0x5f8, value);
    }
    pub fn exit_code(&self) -> u64 {
        self.read64(0x70)
    }
    pub fn exit_info(&self) -> (u64, u64) {
        (self.read64(0x78), self.read64(0x80))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidVmcb;

fn canonical(value: u64) -> bool {
    ((value << 16) as i64 >> 16) as u64 == value
}

fn saved_msr_offset(msr: u32) -> Option<usize> {
    Some(match msr {
        PAT => 0x668,        // Guest PAT, loaded by VMRUN when nested paging is enabled.
        0xc0000081 => 0x600, // STAR
        0xc0000082 => 0x608, // LSTAR
        0xc0000083 => 0x610, // CSTAR
        0xc0000084 => 0x618, // SFMASK
        0xc0000100 => 0x448, // FS base
        0xc0000101 => 0x458, // GS base
        0xc0000102 => 0x620, // Kernel GS base
        0x174 => 0x628,
        0x175 => 0x630,
        0x176 => 0x638,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn guest_pat_is_private_and_rejects_reserved_memory_types() {
        let mut first = Vmcb::new();
        first.initialize(1, 0x1000, 0x2000, 0x4000, 0x8000).unwrap();
        let mut second = Vmcb::new();
        second
            .initialize(2, 0x3000, 0x4000, 0x5000, 0x9000)
            .unwrap();
        let reset = 0x0007_0406_0007_0406;
        assert_eq!(first.saved_msr(PAT), Some(reset));
        for entry in [0, 1, 4, 5, 6, 7] {
            let value = u64::from_le_bytes([entry; 8]);
            assert!(first.set_saved_msr(PAT, value));
            assert_eq!(first.saved_msr(PAT), Some(value));
            assert_eq!(first.0[0x5c], 1);
        }
        let retained = first.saved_msr(PAT);
        for byte in 0..8 {
            for invalid in [2, 3, 8, 0x80, 0xff] {
                let mut value = [6; 8];
                value[byte] = invalid;
                assert!(!first.set_saved_msr(PAT, u64::from_le_bytes(value)));
                assert_eq!(first.saved_msr(PAT), retained);
            }
        }
        assert_eq!(second.saved_msr(PAT), Some(reset));
    }
    #[test]
    fn invalid_guest_msr_writes_cannot_poison_vmload() {
        let mut vmcb = Vmcb::new();
        for index in [
            0xc0000082, 0xc0000083, 0xc0000100, 0xc0000101, 0xc0000102, 0x175, 0x176,
        ] {
            assert!(vmcb.set_saved_msr(index, 0xffff800000001000));
            for invalid in [0x800000000000, 0xffff000000001000] {
                assert!(!vmcb.set_saved_msr(index, invalid));
                assert_eq!(vmcb.saved_msr(index), Some(0xffff800000001000));
            }
        }
        for (index, valid, invalid) in [
            (0xc0000081, 0x001b000800000000, 1),
            (0xc0000084, 0x200, 1 << 32),
            (0x174, 8, 1 << 32),
        ] {
            assert!(vmcb.set_saved_msr(index, valid));
            assert!(!vmcb.set_saved_msr(index, invalid));
            assert_eq!(vmcb.saved_msr(index), Some(valid));
        }
        assert!(!vmcb.set_saved_msr(VM_HSAVE_PA, 0x1000));
        assert_eq!(vmcb.saved_msr(VM_HSAVE_PA), None);
    }
    #[test]
    fn rejects_invalid_translation_roots_and_contexts() {
        let mut vmcb = Vmcb::new();
        for (asid, npt, cr3, entry) in [
            (0, 0x1000, 0x2000, 0),
            (1, 0, 0x2000, 0),
            (1, 0x1001, 0x2000, 0),
            (1, 0x1000, 0x2001, 0),
            (1, 0x1000, 0x2000, 1 << 48),
        ] {
            assert_eq!(
                vmcb.initialize(asid, npt, cr3, entry, 0x8000),
                Err(InvalidVmcb)
            );
        }
        vmcb.initialize(1, 0x1000, 0x2000, 0x4000, 0x8000).unwrap();
        assert_eq!(vmcb.rip(), 0x4000);
        assert_eq!(core::mem::size_of::<Vmcb>(), 4096);
        assert_eq!(core::mem::align_of::<Vmcb>(), 4096);
    }
}
