//! Temporary compatibility platform for the saved standalone Trusty image.
//! Real RAM isolation is active. This bring-up target does not publish product
//! boot evidence or expose physical PCI/DMA devices to its guest.
use crate::{hex, log};
use bexos_secure_monitor::{
    svm::{self, Vmcb},
    vcpu::Registers,
};
use core::arch::asm;
#[path = "platform_state.rs"]
mod state;

pub unsafe fn rdmsr(index: u32) -> u64 {
    let (lo, hi): (u32, u32);
    unsafe {
        asm!("rdmsr", in("ecx") index, out("eax") lo, out("edx") hi, options(nomem, nostack));
    }
    (hi as u64) << 32 | lo as u64
}
pub unsafe fn wrmsr(index: u32, value: u64) {
    unsafe {
        asm!("wrmsr", in("ecx") index, in("eax") value as u32, in("edx") (value >> 32) as u32, options(nomem, nostack));
    }
}
pub struct Platform<const N: usize> {
    pub devices: crate::devices::Devices<N>,
    memory: crate::memory::DomainMemory,
    rpmb: bool,
    console: crate::console::Console,
    rtc: Option<bexos_secure_monitor::cmos::Clock>,
    run: [bexos_secure_monitor::run_state::RunState; N],
}
impl<const N: usize> Platform<N> {
    pub fn new(memory: crate::memory::DomainMemory, rpmb: bool) -> Self {
        let mut platform = Self::for_restore(memory, rpmb);
        if !rpmb {
            platform.rtc = Some(unsafe { crate::rtc::snapshot() });
        }
        platform
    }
    /// Private preparation only. Restoring an owner must not sample physical
    /// CMOS or reinitialize hardware before importing the retained state.
    pub(crate) fn for_restore(memory: crate::memory::DomainMemory, rpmb: bool) -> Self {
        Self {
            rtc: None,
            run: [bexos_secure_monitor::run_state::RunState::default(); N],
            devices: crate::devices::Devices::new(),
            memory,
            rpmb,
            console: crate::console::Console::new(rpmb),
        }
    }
    pub fn reset(&mut self, cpu: usize) {
        self.run[cpu].reset();
    }
    pub fn memory_base(&self) -> u64 {
        self.memory.base
    }
    pub fn before_entry(&mut self, cpu: usize, vmcb: &mut Vmcb, now: u64) -> bool {
        self.devices.before_entry(cpu, vmcb, now);
        self.run[cpu].ready(vmcb.virtual_interrupt_pending(), vmcb.rflags() & 0x200 != 0)
    }
    pub unsafe fn exit(&mut self, cpu: usize, vmcb: &mut Vmcb, regs: &mut Registers) -> bool {
        if cpu >= N {
            return false;
        }
        match vmcb.exit_code() {
            svm::EXIT_VMMCALL => {
                unsafe {
                    crate::transport::exit(self.rpmb, self.memory.base, vmcb, regs);
                }
                true
            }
            svm::EXIT_NPF => unsafe {
                self.devices.mmio(
                    cpu,
                    self.memory,
                    vmcb,
                    regs,
                    bexos_secure_monitor::clock::now_ns(),
                )
            },
            svm::EXIT_NMI => true,
            svm::EXIT_HLT => {
                vmcb.complete_halt();
                self.run[cpu].halt();
                true
            }
            svm::EXIT_CPUID => {
                let leaf = vmcb.rax() as u32;
                if leaf == bexos_secure_monitor_abi::transport::DISCOVERY_LEAF {
                    vmcb.set_rax(u64::from(
                        bexos_secure_monitor_abi::transport::DISCOVERY_MAGIC,
                    ));
                    regs.rbx = u64::from(bexos_secure_monitor_abi::VERSION);
                    regs.rcx = u64::from(bexos_secure_monitor_abi::ARCH_X86_64);
                    regs.rdx = if cfg!(feature = "secure_product") {
                        u64::from(bexos_secure_monitor_abi::transport::DISCOVERY_BOOT_SERVICES)
                    } else {
                        0
                    } | if cfg!(feature = "resident_nucleus") {
                        u64::from(bexos_secure_monitor_abi::transport::DISCOVERY_ROOT_CONTROL)
                    } else {
                        0
                    };
                    vmcb.set_rip(vmcb.rip() + 2);
                    return true;
                }
                let result = core::arch::x86_64::__cpuid_count(leaf, regs.rcx as u32);
                let result = bexos_secure_monitor::cpu_policy::filter(
                    leaf,
                    cpu as u8,
                    N as u8,
                    [result.eax, result.ebx, result.ecx, result.edx],
                );
                vmcb.set_rax(u64::from(result[0]));
                regs.rbx = u64::from(result[1]);
                regs.rcx = u64::from(result[2]);
                regs.rdx = u64::from(result[3]);
                vmcb.set_rip(vmcb.rip() + 2);
                true
            }
            // CR4 writes carry the source register in EXITINFO1 when decode
            // assists are available. Handle only the legacy feature set saved
            // by our entry routine; enabling XSAVE is explicitly rejected.
            0x14 => {
                let (code, length) = unsafe { self.memory.instruction(vmcb) };
                let (prefix, start) = if vmcb.long_code() && code[0] & 0xf0 == 0x40 {
                    (code[0], 1)
                } else {
                    (0, 0)
                };
                if length < start + 3
                    || code[start..start + 2] != [0x0f, 0x22]
                    || code[start + 2] & 0xf8 != 0xe0
                    || prefix & 4 != 0
                {
                    return false;
                }
                let source = (code[start + 2] & 7) | ((prefix & 1) << 3);
                let value = match source {
                    0 => vmcb.rax(),
                    1 => regs.rcx,
                    2 => regs.rdx,
                    3 => regs.rbx,
                    4 => vmcb.rsp(),
                    5 => regs.rbp,
                    6 => regs.rsi,
                    7 => regs.rdi,
                    8 => regs.r8,
                    9 => regs.r9,
                    10 => regs.r10,
                    11 => regs.r11,
                    12 => regs.r12,
                    13 => regs.r13,
                    14 => regs.r14,
                    15 => regs.r15,
                    _ => return false,
                };
                if value & !0x6f0 != 0 {
                    return false;
                }
                vmcb.set_cr4(value);
                vmcb.set_rip(vmcb.rip() + start as u64 + 3);
                true
            }
            svm::EXIT_MSR => {
                let index = regs.rcx as u32;
                let write = vmcb.exit_info().0 & 1 != 0;
                let value = (regs.rdx as u32 as u64) << 32 | vmcb.rax() as u32 as u64;
                if write {
                    if index == svm::EFER {
                        if value & !(0xd01 | svm::SVME) != 0 {
                            return false;
                        }
                        vmcb.set_efer(value | svm::SVME);
                    } else if index == 0x1b {
                        if value != (0xfee00800 | if cpu == 0 { 0x100 } else { 0 }) {
                            return false;
                        }
                    } else if !vmcb.set_saved_msr(index, value) {
                        log("monitor-runtime: unknown MSR write=");
                        hex(index as u64);
                        return false;
                    }
                } else {
                    let value = if index == svm::EFER {
                        vmcb.efer() & !svm::SVME
                    } else if index == 0x1b {
                        0xfee00800 | if cpu == 0 { 0x100 } else { 0 }
                    } else if let Some(value) = vmcb.saved_msr(index) {
                        value
                    } else {
                        log("monitor-runtime: unknown MSR read=");
                        hex(index as u64);
                        return false;
                    };
                    vmcb.set_rax(value as u32 as u64);
                    regs.rdx = value >> 32;
                }
                vmcb.set_rip(vmcb.rip() + 2);
                true
            }
            svm::EXIT_IOIO => {
                let info = vmcb.exit_info().0;
                let port = (info >> 16) as u16;
                let input = info & 1 != 0;
                // Scalar byte I/O only; never execute guest string/REP I/O.
                if info & 12 != 0 || (info >> 4) & 7 != 1 {
                    return false;
                }
                if matches!(port, 0x70 | 0x71) {
                    let Some(rtc) = self.rtc.as_mut() else {
                        return false;
                    };
                    let now = unsafe { bexos_secure_monitor::clock::now_ns() };
                    if input {
                        let Some(value) = rtc.read(port, now) else {
                            return false;
                        };
                        vmcb.set_rax((vmcb.rax() & !255) | u64::from(value));
                    } else if !rtc.write(port, vmcb.rax() as u8, now) {
                        return false;
                    }
                    vmcb.set_rip(vmcb.exit_info().1);
                    return true;
                }
                if (0x3f8..=0x3ff).contains(&port) {
                    if input {
                        vmcb.set_rax(
                            (vmcb.rax() & !255) | u64::from(self.console.read(port - 0x3f8)),
                        );
                    } else {
                        self.console.write(port - 0x3f8, vmcb.rax() as u8);
                    }
                    vmcb.set_rip(vmcb.exit_info().1);
                    return true;
                }
                if matches!(port, 0x20 | 0x21 | 0xa0 | 0xa1 | 0x40..=0x43) {
                    let now = unsafe { bexos_secure_monitor::clock::now_ns() };
                    if input {
                        let Some(value) = self.devices.fabric.legacy.read(port, now) else {
                            return false;
                        };
                        vmcb.set_rax((vmcb.rax() & !255) | u64::from(value));
                    } else if !self
                        .devices
                        .fabric
                        .legacy
                        .write(port, vmcb.rax() as u8, now)
                    {
                        return false;
                    }
                    vmcb.set_rip(vmcb.exit_info().1);
                    return true;
                }
                if !matches!(port, 0x20 | 0x21 | 0xa0 | 0xa1 | 0x40..=0x43 | 0x3f8..=0x3ff | 0x2f8..=0x2ff | 0x80)
                {
                    log("monitor-runtime: forbidden port=");
                    hex(port as u64);
                    return false;
                }
                if (0x2f8..=0x2ff).contains(&port) && !self.rpmb {
                    return false;
                }
                #[cfg(any(
                    feature = "trusty_recovery_probe",
                    all(feature = "resident_nucleus", feature = "secure_product")
                ))]
                if let Some(value) = crate::secure_boot::fenced_rpmb_port(
                    self.memory.base,
                    port,
                    input,
                    vmcb.rax() as u8,
                ) {
                    if input {
                        vmcb.set_rax((vmcb.rax() & !255) | u64::from(value));
                    }
                    vmcb.set_rip(vmcb.exit_info().1);
                    return true;
                }
                unsafe {
                    if input {
                        let value: u8;
                        asm!("in al, dx", in("dx") port, out("al") value, options(nomem, nostack));
                        vmcb.set_rax((vmcb.rax() & !255) | value as u64);
                    } else {
                        asm!("out dx, al", in("dx") port, in("al") vmcb.rax() as u8, options(nomem, nostack));
                    }
                }
                vmcb.set_rip(vmcb.exit_info().1);
                true
            }
            _ => false,
        }
    }
}
