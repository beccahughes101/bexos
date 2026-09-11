#![no_std]
#![no_main]

mod contexts;
#[cfg(feature = "dma_probe")]
mod dma;
#[cfg(feature = "firmware_disk_probe")]
mod firmware_disk;
mod interrupts;
mod permissions;

use bexos_secure_monitor::{
    npt::{DomainMap, Page},
    svm::{self, Vmcb},
    vcpu::{Registers, bexos_svm_enter},
};
use core::{
    arch::{asm, global_asm},
    panic::PanicInfo,
};

global_asm!(
    r#"
.section .text.boot,"ax"
.global _start
_start:
    lea rsp, [rip + probe_stack_end]
    and rsp, -16
    call probe_main
    ud2
.section .bss,"aw",@nobits
.balign 16
probe_stack:
.skip 32768
probe_stack_end:
"#
);

#[repr(C, align(2097152))]
struct GuestMemory([u8; 2097152]);
#[repr(C, align(4096))]
struct PermissionMaps([u8; 12288]);
static mut GUEST: GuestMemory = GuestMemory([0; 2097152]);
static mut NPT: DomainMap = DomainMap::new();
static mut VMCB: Vmcb = Vmcb::new();
static mut HOST_VMCB: Vmcb = Vmcb::new();
static mut HSAVE: Page = Page::zeroed();
static mut IOPM: PermissionMaps = PermissionMaps([0xff; 12288]);
static mut MSRPM: PermissionMaps = PermissionMaps([0xff; 12288]);

fn log(text: &str) {
    for byte in text.bytes() {
        unsafe {
            asm!("out dx, al", in("dx") 0x3f8u16, in("al") byte, options(nomem, nostack));
        }
    }
}
fn hex(value: u64) {
    for shift in (0..16).rev() {
        let byte = b"0123456789abcdef"[((value >> (shift * 4)) & 15) as usize];
        unsafe {
            asm!("out dx, al", in("dx") 0x3f8u16, in("al") byte, options(nomem, nostack));
        }
    }
    log("\n");
}
unsafe fn rdmsr(index: u32) -> u64 {
    let (lo, hi): (u32, u32);
    unsafe {
        asm!("rdmsr", in("ecx") index, out("eax") lo, out("edx") hi, options(nomem, nostack));
    }
    (hi as u64) << 32 | lo as u64
}
unsafe fn wrmsr(index: u32, value: u64) {
    unsafe {
        asm!("wrmsr", in("ecx") index, in("eax") value as u32, in("edx") (value >> 32) as u32, options(nomem, nostack));
    }
}
#[unsafe(no_mangle)]
extern "C" fn probe_main() -> ! {
    log("svm-probe: root entered\n");
    unsafe {
        #[cfg(feature = "firmware_disk_probe")]
        firmware_disk::verify();
        let features = core::arch::x86_64::__cpuid(0x80000001);
        let svm_features = core::arch::x86_64::__cpuid(0x8000000a);
        if features.ecx & (1 << 2) == 0 || svm_features.edx & 1 == 0 {
            log("svm-probe: SVM/NPT unavailable; refusing execution\n");
            loop {
                asm!("cli; hlt", options(nomem, nostack));
            }
        }
        wrmsr(svm::EFER, rdmsr(svm::EFER) | svm::SVME);
        wrmsr(svm::VM_HSAVE_PA, core::ptr::addr_of!(HSAVE) as u64);
        let memory = &mut *core::ptr::addr_of_mut!(GUEST);
        let npt = &mut *core::ptr::addr_of_mut!(NPT);
        let vmcb = &mut *core::ptr::addr_of_mut!(VMCB);
        let mut registers = Registers::default();
        npt.map_probe_memory(memory as *mut _ as u64).unwrap();
        for (offset, value) in [
            (0x1000, 0x2007u64),
            (0x2000, 0x3007),
            (0x3000, 0x87),
            (0x3008, 0x200087),
        ] {
            memory.0[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
        }
        // The guest reports a value, then reads a valid guest-virtual mapping
        // whose guest-physical backing is deliberately absent from its NPT.
        let code = [
            0x48, 0xc7, 0xc0, 0x34, 0x12, 0, 0, 0x0f, 0x01, 0xd9, 0x48, 0xa1, 0, 0, 0x20, 0, 0, 0,
            0, 0, 0xf4,
        ];
        memory.0[0x4000..0x4000 + code.len()].copy_from_slice(&code);
        vmcb.initialize(1, npt as *mut _ as u64, 0x1000, 0x4000, 0x10000)
            .unwrap();
        vmcb.set_permission_maps(
            core::ptr::addr_of!(IOPM) as u64,
            core::ptr::addr_of!(MSRPM) as u64,
        )
        .unwrap();
        bexos_svm_enter(
            vmcb as *mut _ as u64,
            &mut registers,
            core::ptr::addr_of!(HOST_VMCB) as u64,
        );
        log("svm-probe: exit=");
        hex(vmcb.exit_code());
        assert_eq!(vmcb.exit_code(), svm::EXIT_VMMCALL);
        assert_eq!(vmcb.rax(), 0x1234);
        log("svm-probe: guest hypercall verified\n");
        vmcb.set_rip(vmcb.rip() + 3); // VMMCALL has a fixed three-byte encoding.
        bexos_svm_enter(
            vmcb as *mut _ as u64,
            &mut registers,
            core::ptr::addr_of!(HOST_VMCB) as u64,
        );
        log("svm-probe: isolation exit=");
        hex(vmcb.exit_code());
        assert_eq!(vmcb.exit_code(), svm::EXIT_NPF);
        assert_eq!(vmcb.exit_info().1, 0x200000);
        log("svm-probe: nested-page isolation verified\n");
        // Try privileged device/MSR controls and a nested VMRUN. Each must
        // exit before the instruction can change monitor-owned state.
        for (offset, code, expected) in [
            (0x4100, &[0xe6, 0x80][..], svm::EXIT_IOIO),
            (
                0x4200,
                &[0xb9, 0x80, 0, 0, 0xc0, 0x0f, 0x32][..],
                svm::EXIT_MSR,
            ),
            (
                0x4300,
                &[0xb8, 0, 0x10, 0, 0, 0x0f, 0x01, 0xd8][..],
                svm::EXIT_VMRUN,
            ),
        ] {
            memory.0[offset..offset + code.len()].copy_from_slice(code);
            vmcb.set_rip(offset as u64);
            bexos_svm_enter(
                vmcb as *mut _ as u64,
                &mut registers,
                core::ptr::addr_of!(HOST_VMCB) as u64,
            );
            log("svm-probe: privileged instruction exit=");
            hex(vmcb.exit_code());
            assert_eq!(vmcb.exit_code(), expected);
        }
        log("svm-probe: device MSR and nested virtualization intercepted\n");
        permissions::verify();
        interrupts::verify();
        contexts::verify();
        #[cfg(feature = "dma_probe")]
        dma::verify();
        log("svm-probe: independent guest register contexts verified\n");
    }
    loop {
        unsafe {
            asm!("cli; hlt", options(nomem, nostack));
        }
    }
}
#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    log("svm-probe: FAILED\n");
    loop {
        unsafe {
            asm!("cli; hlt", options(nomem, nostack));
        }
    }
}
