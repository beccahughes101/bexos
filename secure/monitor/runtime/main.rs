#![no_std]
#![no_main]

#[cfg(feature = "external_payload")]
mod payload;
#[cfg(feature = "rollback_fixture")]
mod rollback_fixture;

#[cfg(feature = "boot_approval")]
mod approval;
#[cfg(feature = "checkpoint_probe")]
mod checkpoint_probe;
mod console;
mod devices;
#[cfg(feature = "normal_world")]
mod domain_state;
#[cfg(feature = "secure_product")]
mod firmware_generations;
#[cfg(feature = "firmware_recovery_probe")]
mod firmware_recovery;
#[cfg(feature = "normal_world")]
mod handoff;
mod memory;
#[cfg(feature = "monitor_transfer")]
mod monitor_transfer;
#[cfg(feature = "normal_world")]
mod normal;
#[cfg(feature = "resident_nucleus")]
mod nucleus;
#[cfg(feature = "nucleus_probe")]
mod nucleus_probe;
mod pci;
mod platform;
#[cfg(all(feature = "resident_nucleus", feature = "secure_product"))]
mod product_recovery;
#[cfg(any(
    feature = "firmware_recovery_probe",
    feature = "trusty_recovery_probe",
    feature = "nucleus_probe"
))]
mod recovery_faults;
#[cfg(feature = "secure_product")]
mod replacement;
mod root;
mod rtc;
#[cfg(feature = "normal_world")]
mod schedule;
#[cfg(any(
    feature = "trusty_recovery_probe",
    all(feature = "resident_nucleus", feature = "secure_product")
))]
mod secure_boot;
mod secure_state;
#[cfg(feature = "selection_probe")]
mod selection_probe;
mod transport;
#[cfg(feature = "trusty_recovery_probe")]
mod trusty_recovery;
use bexos_secure_monitor::{
    image::Image,
    npt::Page,
    npt_tables::{PageSize, PageTables, Permissions, RamBank},
    svm::{self, Vmcb},
    vcpu::{Registers, bexos_svm_enter},
};
use core::{
    arch::{asm, global_asm},
    panic::PanicInfo,
};

const BANK: u64 = 0x20000000;
const BANK_SIZE: usize = 0x10000000;
static TRUSTY: &[u8] = include_bytes!(env!("TRUSTY_ELF"));
#[unsafe(link_section = ".resident.trusty_npt")]
static mut TABLES: PageTables<16> = PageTables::empty();
#[unsafe(link_section = ".resident.trusty_cpu")]
static mut VCPU: Vmcb = Vmcb::new();
#[unsafe(link_section = ".resident.host")]
static mut HOST: Vmcb = Vmcb::new();
#[unsafe(link_section = ".resident.hsave")]
static mut HSAVE: Page = Page::zeroed();
#[repr(C, align(4096))]
struct IoMaps([u8; 12288]);
#[unsafe(link_section = ".resident.iopm")]
static mut IOPM: IoMaps = IoMaps([0xff; 12288]);
#[unsafe(link_section = ".resident.msrpm")]
static mut MSRPM: IoMaps = IoMaps([0xff; 12288]);

global_asm!(
    r#"
.section .text.boot,"ax"
.global _start
_start:
    cli
    cld
    lea rsp, [rip + runtime_stack_end]
    and rsp, -16
    call runtime_main
    ud2
.section .bss,"aw",@nobits
.balign 16
runtime_stack:
.skip {stack_bytes}
.global runtime_stack_end
runtime_stack_end:
"#,
    // The aggregate diagnostic nests secure and four-CPU normal snapshots while
    // retaining its comparison copy. Its measured optimized call chain exceeds
    // the ordinary 512 KiB bootstrap stack; product boot does not take this path.
    stack_bytes = const if cfg!(feature = "normal_checkpoint_probe") {
        1024 * 1024
    } else {
        512 * 1024
    },
);

pub fn log(text: &str) {
    for byte in text.bytes() {
        unsafe {
            asm!("out dx, al", in("dx") 0x3f8u16, in("al") byte, options(nomem, nostack));
        }
    }
}
pub fn hex(value: u64) {
    for shift in (0..16).rev() {
        let byte = b"0123456789abcdef"[((value >> (shift * 4)) & 15) as usize];
        unsafe {
            asm!("out dx, al", in("dx") 0x3f8u16, in("al") byte, options(nomem, nostack));
        }
    }
    log("\n");
}
#[unsafe(no_mangle)]
extern "C" fn runtime_main(entry_header: u64, entry_flags: u64, entry_reserved: u64) -> ! {
    log("monitor-runtime: entering assigned Trusty domain\n");
    let authenticated_entry = bexos_secure_monitor_abi::boot::authenticated_entry(
        entry_header,
        entry_flags,
        entry_reserved,
    );
    if cfg!(feature = "require_efi") && !authenticated_entry {
        log("monitor-runtime: authenticated EFI entry required; refusing execution\n");
        halt();
    }
    unsafe {
        root::initialize();
        #[cfg(feature = "boot_approval")]
        let verified = approval::verify();
        let features = core::arch::x86_64::__cpuid(0x80000001);
        let svm_features = core::arch::x86_64::__cpuid(0x8000000a);
        if features.ecx & (1 << 2) == 0 || svm_features.edx & 1 == 0 {
            log("monitor-runtime: SVM/NPT unavailable; refusing execution\n");
            halt();
        }
        platform::wrmsr(svm::EFER, platform::rdmsr(svm::EFER) | svm::SVME);
        platform::wrmsr(svm::VM_HSAVE_PA, core::ptr::addr_of!(HSAVE) as u64);
        assert!(bexos_secure_monitor::clock::initialize());
        let start = bexos_secure_monitor::clock::now_ns();
        loop {
            let status: u8;
            asm!("in al, dx", in("dx") 0x2feu16, out("al") status, options(nomem, nostack));
            if status & 0x80 != 0 {
                break;
            }
            if bexos_secure_monitor::clock::now_ns().saturating_sub(start) > 10_000_000_000 {
                log("monitor-runtime: RPMB transport unavailable; refusing execution\n");
                halt();
            }
        }
        let image = Image::parse(TRUSTY, BANK_SIZE).unwrap();
        let memory = core::slice::from_raw_parts_mut(BANK as *mut u8, BANK_SIZE);
        memory.fill(0);
        image.load(memory).unwrap();
        let tables = &mut *core::ptr::addr_of_mut!(TABLES);
        tables
            .initialize(
                core::ptr::addr_of!(TABLES) as u64,
                RamBank {
                    start: BANK,
                    length: BANK_SIZE as u64,
                },
            )
            .unwrap();
        for page in 0..128 {
            tables
                .map(
                    page * 0x200000,
                    BANK + page * 0x200000,
                    PageSize::Large,
                    Permissions::RAM,
                )
                .unwrap();
        }
        // Device pages stay absent from NPT. Each access traps to the
        // monitor's per-domain controller; physical controllers stay private.
        for (offset, value) in [(0x1000, 0x2007u64), (0x2000, 0x3007)] {
            memory[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
        }
        for page in 0..512 {
            memory[0x3000 + page * 8..0x3008 + page * 8]
                .copy_from_slice(&(page as u64 * 0x200000 | 0x87).to_le_bytes());
        }
        let vmcb = &mut *core::ptr::addr_of_mut!(VCPU);
        vmcb.initialize(1, tables.root().unwrap(), 0x1000, image.entry(), 0x10000)
            .unwrap();
        vmcb.set_permission_maps(
            core::ptr::addr_of!(IOPM) as u64,
            core::ptr::addr_of!(MSRPM) as u64,
        )
        .unwrap();
        vmcb.intercept_cpuid();
        vmcb.intercept_nmi();
        transport::initialize();
        #[cfg(feature = "normal_world")]
        let mut normal = normal::Normal::initialize();
        bexos_secure_monitor::watchdog::initialize();
        let mut regs = Registers::default();
        let mut platform = platform::Platform::<1>::new(
            memory::DomainMemory {
                base: BANK,
                length: BANK_SIZE,
            },
            true,
        );
        #[cfg(feature = "checkpoint_probe")]
        checkpoint_probe::verify(vmcb, &mut regs, &mut platform);
        #[cfg(all(feature = "boot_ipc_probe", not(feature = "boot_approval")))]
        {
            let transport = transport::Boot::cold(|| secure_step(vmcb, &mut regs, &mut platform));
            let mut avb = bexos_trusty_boot::avb::Avb::connect(transport)
                .expect("real Trusty AVB connection");
            let floor = avb.read_rollback(0).expect("real Trusty rollback reply");
            avb.close().expect("release boot QL device");
            log("monitor-runtime: Trusty AVB rollback floor=\n");
            hex(floor);
            log("monitor-runtime: Trusty AVB IPC verified\n");
        }
        #[cfg(all(feature = "resident_nucleus", feature = "secure_product"))]
        let mut monitor = product_recovery::select(vmcb, &mut regs, &mut platform);
        #[cfg(feature = "boot_approval")]
        let boot_approval = approval::approve(
            transport::Boot::cold(|| secure_step(vmcb, &mut regs, &mut platform)),
            verified,
        );
        #[cfg(feature = "selection_probe")]
        selection_probe::verify(&mut transport::Boot::new(|| {
            secure_step(vmcb, &mut regs, &mut platform)
        }));
        #[cfg(all(
            feature = "monitor_transfer",
            not(feature = "monitor_candidate"),
            not(feature = "firmware_recovery_probe"),
            not(feature = "normal_world")
        ))]
        monitor_transfer::transfer(vmcb, &mut regs, &mut platform);
        #[cfg(all(
            feature = "firmware_recovery_probe",
            not(feature = "monitor_candidate")
        ))]
        firmware_recovery::boot(vmcb, &mut regs, &mut platform);
        #[cfg(feature = "trusty_recovery_probe")]
        trusty_recovery::boot(vmcb, &mut regs, &mut platform);
        #[cfg(feature = "boot_release")]
        {
            use bexos_trusty_boot::ql::Transport;
            let mut owner = transport::Boot::new(|| secure_step(vmcb, &mut regs, &mut platform));
            assert_eq!(
                owner.exchange(
                    bexos_secure_monitor_abi::transport::BOOT_OWNER_OPERATION_BASE,
                    1,
                    &mut [1],
                ),
                Ok(0)
            );
            log("monitor-runtime: boot RPMB owner release verified\n");
        }
        #[cfg(feature = "secure_product")]
        normal.approve_entry(verified, boot_approval);
        #[cfg(all(feature = "resident_nucleus", not(feature = "secure_product")))]
        let mut monitor = nucleus::Monitor::initialize();
        #[cfg(feature = "nucleus_probe")]
        let mut exercised = false;
        loop {
            #[cfg(feature = "nucleus_probe")]
            if !exercised && {
                #[cfg(feature = "normal_world")]
                {
                    normal.all_running_for_probe()
                }
                #[cfg(not(feature = "normal_world"))]
                {
                    true
                }
            } {
                nucleus_probe::exercise(
                    &mut monitor,
                    #[cfg(feature = "normal_world")]
                    &mut normal,
                    vmcb,
                    &mut regs,
                    &mut platform,
                );
                exercised = true;
            }
            #[cfg(feature = "resident_nucleus")]
            {
                #[cfg(feature = "secure_product")]
                {
                    replacement::poll();
                    replacement::activation::run(
                        &mut monitor,
                        &mut normal,
                        vmcb,
                        &mut regs,
                        &mut platform,
                    );
                }
                assert!(
                    monitor.turn(
                        #[cfg(feature = "normal_world")]
                        &mut normal,
                        vmcb,
                        &mut regs,
                        &mut platform,
                        150_000_000
                    ),
                    "committed monitor failed; recovery required"
                );
                continue;
            }
            #[cfg(all(
                feature = "monitor_transfer",
                not(feature = "monitor_candidate"),
                feature = "normal_world"
            ))]
            if normal.all_running_for_probe() {
                monitor_transfer::transfer(&mut normal, vmcb, &mut regs, &mut platform);
            }
            #[cfg(feature = "secure_product")]
            replacement::poll();
            #[cfg(feature = "normal_world")]
            schedule::normal_turn(&mut normal);
            #[cfg(feature = "normal_checkpoint_probe")]
            domain_state::probe(&mut normal, vmcb, &mut regs, &mut platform);
            secure_step(vmcb, &mut regs, &mut platform);
        }
    }
}
unsafe fn secure_step(vmcb: &mut Vmcb, regs: &mut Registers, platform: &mut platform::Platform<1>) {
    unsafe {
        if !platform.before_entry(0, vmcb, bexos_secure_monitor::clock::now_ns()) {
            bexos_secure_monitor::watchdog::service_pending();
            return;
        }
        #[cfg(feature = "monitor_transfer")]
        bexos_secure_monitor::watchdog::disarm_recovery();
        bexos_svm_enter(
            vmcb as *mut _ as u64,
            regs,
            core::ptr::addr_of!(HOST) as u64,
        );
        platform.devices.after_exit(0, vmcb);
        bexos_secure_monitor::watchdog::service_pending();
        if !platform.exit(0, vmcb, regs) {
            log("monitor-runtime: rejected exit=");
            hex(vmcb.exit_code());
            log("monitor-runtime: info1=");
            hex(vmcb.exit_info().0);
            log("monitor-runtime: info2=");
            hex(vmcb.exit_info().1);
            log("monitor-runtime: rip=");
            hex(vmcb.rip());
            log("monitor-runtime: rax=");
            hex(vmcb.rax());
            halt();
        }
        #[cfg(feature = "resident_nucleus")]
        nucleus::completed_boundary(1);
    }
}
fn halt() -> ! {
    loop {
        unsafe {
            asm!("cli; hlt", options(nomem, nostack));
        }
    }
}
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    log("monitor-runtime: FAILED\n");
    if let Some(location) = info.location() {
        log(location.file());
        hex(location.line() as u64);
    }
    halt()
}
