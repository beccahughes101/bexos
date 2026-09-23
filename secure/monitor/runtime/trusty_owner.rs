//! Second Trusty execution context owned by the permanent x86 nucleus.
//! Candidate code can touch only its inactive RAM bank and private CPU/device
//! state. The authoritative transport is switched only at a completed exit.
use crate::{memory::DomainMemory, platform::Platform};
use bexos_secure_monitor::{
    image::{Image, ImageError},
    npt_tables::{PageSize, PageTables, Permissions, RamBank},
    svm::Vmcb,
    vcpu::Registers,
};

pub const SECOND_BANK: u64 = 0x6000_0000;
#[repr(C, align(4096))]
struct CandidateCpu(Vmcb);
static mut CANDIDATE_CPU: CandidateCpu = CandidateCpu(Vmcb::new());
static mut SECOND_TABLES: PageTables<16> = PageTables::empty();

pub struct Candidate {
    bank: u64,
    source_bank: u64,
    registers: Registers,
    platform: Platform<1>,
    active: bool,
    failed: bool,
}

fn tables(bank: u64) -> Option<&'static mut PageTables<16>> {
    unsafe {
        match bank {
            crate::BANK => Some(&mut *core::ptr::addr_of_mut!(crate::TABLES)),
            SECOND_BANK => Some(&mut *core::ptr::addr_of_mut!(SECOND_TABLES)),
            _ => None,
        }
    }
}

pub fn table_root(bank: u64) -> Option<u64> {
    tables(bank)?.root().ok()
}

impl Candidate {
    pub unsafe fn prepare(bytes: &[u8], source_bank: u64) -> Result<Self, ImageError> {
        let bank = match source_bank {
            crate::BANK => SECOND_BANK,
            SECOND_BANK => crate::BANK,
            _ => return Err(ImageError::Destination),
        };
        let image = Image::parse(bytes, crate::BANK_SIZE)?;
        let memory = unsafe { core::slice::from_raw_parts_mut(bank as *mut u8, crate::BANK_SIZE) };
        memory.fill(0);
        image.load(memory)?;
        let tables = tables(bank).ok_or(ImageError::Destination)?;
        let tables_physical = tables as *const _ as u64;
        tables
            .initialize(
                tables_physical,
                RamBank {
                    start: bank,
                    length: crate::BANK_SIZE as u64,
                },
            )
            .map_err(|_| ImageError::Destination)?;
        for page in 0..crate::BANK_SIZE as u64 / 0x20_0000 {
            tables
                .map(
                    page * 0x20_0000,
                    bank + page * 0x20_0000,
                    PageSize::Large,
                    Permissions::RAM,
                )
                .map_err(|_| ImageError::Destination)?;
        }
        for (offset, value) in [(0x1000, 0x2007u64), (0x2000, 0x3007)] {
            memory[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
        }
        for page in 0..512 {
            memory[0x3000 + page * 8..0x3008 + page * 8]
                .copy_from_slice(&(page as u64 * 0x20_0000 | 0x87).to_le_bytes());
        }
        let cpu = unsafe { &mut (*core::ptr::addr_of_mut!(CANDIDATE_CPU)).0 };
        *cpu = Vmcb::new();
        let asid = if bank == crate::BANK { 1 } else { 2 };
        cpu.initialize(asid, tables.root().unwrap(), 0x1000, image.entry(), 0x10000)
            .map_err(|_| ImageError::Destination)?;
        cpu.set_permission_maps(
            core::ptr::addr_of!(crate::IOPM) as u64,
            core::ptr::addr_of!(crate::MSRPM) as u64,
        )
        .map_err(|_| ImageError::Destination)?;
        cpu.intercept_cpuid();
        cpu.intercept_nmi();
        Ok(Self {
            bank,
            source_bank,
            registers: Registers::default(),
            platform: Platform::new(
                DomainMemory {
                    base: bank,
                    length: crate::BANK_SIZE,
                },
                true,
            ),
            active: false,
            failed: false,
        })
    }

    pub fn bank(&self) -> u64 {
        self.bank
    }

    pub unsafe fn step(&mut self) {
        if self.failed {
            return;
        }
        let cpu = unsafe { &mut (*core::ptr::addr_of_mut!(CANDIDATE_CPU)).0 };
        self.failed =
            !unsafe { crate::try_secure_step(cpu, &mut self.registers, &mut self.platform) };
    }

    pub fn healthy(&self) -> bool {
        !self.failed
    }

    pub unsafe fn activate(
        &mut self,
        active_cpu: &mut Vmcb,
        active_registers: &mut Registers,
        active_platform: &mut Platform<1>,
    ) {
        assert!(!self.active && active_platform.memory_base() == self.source_bank);
        let candidate_cpu = unsafe { &mut (*core::ptr::addr_of_mut!(CANDIDATE_CPU)).0 };
        core::mem::swap(active_cpu, candidate_cpu);
        core::mem::swap(active_registers, &mut self.registers);
        core::mem::swap(active_platform, &mut self.platform);
        unsafe {
            crate::transport::activate_candidate(self.bank);
        }
        self.active = true;
    }

    pub unsafe fn rollback(
        &mut self,
        active_cpu: &mut Vmcb,
        active_registers: &mut Registers,
        active_platform: &mut Platform<1>,
    ) {
        assert!(self.active && active_platform.memory_base() == self.bank);
        let source_cpu = unsafe { &mut (*core::ptr::addr_of_mut!(CANDIDATE_CPU)).0 };
        core::mem::swap(active_cpu, source_cpu);
        core::mem::swap(active_registers, &mut self.registers);
        core::mem::swap(active_platform, &mut self.platform);
        unsafe {
            crate::transport::rollback_candidate(self.source_bank);
        }
        self.active = false;
    }

    /// Call after authenticated commitment. The source CPU is no longer
    /// runnable, and its entire private bank is overwritten before reuse.
    pub unsafe fn reclaim(mut self) {
        assert!(self.active);
        let old = unsafe {
            core::slice::from_raw_parts_mut(self.source_bank as *mut u8, crate::BANK_SIZE)
        };
        old.fill(0);
        self.registers = Registers::default();
        unsafe {
            (*core::ptr::addr_of_mut!(CANDIDATE_CPU)).0 = Vmcb::new();
        }
    }
}

impl Drop for Candidate {
    fn drop(&mut self) {
        if !self.active {
            unsafe {
                core::slice::from_raw_parts_mut(self.bank as *mut u8, crate::BANK_SIZE).fill(0);
                (*core::ptr::addr_of_mut!(CANDIDATE_CPU)).0 = Vmcb::new();
            }
        }
    }
}
