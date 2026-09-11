//! Exercise production page-table permissions with actual guest instructions.
use crate::{GUEST, HOST_VMCB, IOPM, MSRPM, log};
use bexos_secure_monitor::{
    npt_tables::{PageSize, PageTables, Permissions, RamBank},
    svm::{self, Vmcb},
    vcpu::{Registers, bexos_svm_enter},
};

static mut TABLES: PageTables<8> = PageTables::empty();
static mut VCPU: Vmcb = Vmcb::new();

pub unsafe fn verify() {
    unsafe {
        let memory = &mut *core::ptr::addr_of_mut!(GUEST);
        let tables = &mut *core::ptr::addr_of_mut!(TABLES);
        let vmcb = &mut *core::ptr::addr_of_mut!(VCPU);
        let backing = core::ptr::addr_of!(GUEST) as u64;
        tables
            .initialize(
                core::ptr::addr_of!(TABLES) as u64,
                RamBank {
                    start: backing,
                    length: 0x200000,
                },
            )
            .unwrap();
        for page in [0x1000, 0x2000, 0x3000, 0x5000, 0xf000] {
            tables
                .map(page, backing + page, PageSize::Small, Permissions::DATA)
                .unwrap();
        }
        tables
            .map(0x4000, backing + 0x4000, PageSize::Small, Permissions::CODE)
            .unwrap();
        tables
            .map(0x6000, backing + 0x6000, PageSize::Small, Permissions::READ)
            .unwrap();
        // Guest first-level tables permit every operation; only NPT denies it.
        for (offset, value) in [(0x1000, 0x2007u64), (0x2000, 0x3007), (0x3000, 0x87)] {
            memory.0[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
        }
        memory.0[0x5000] = 0xf4;
        memory.0[0x6000..0x6008].copy_from_slice(&0xdecafbad_u64.to_le_bytes());
        for (code, address, access) in [
            // mov rax, 6000; mov qword [rax], 1
            (
                &[0x48, 0xc7, 0xc0, 0, 0x60, 0, 0, 0x48, 0xc7, 0, 1, 0, 0, 0][..],
                0x6000,
                2,
            ),
            // mov rax, 5000; jmp rax
            (
                &[0x48, 0xc7, 0xc0, 0, 0x50, 0, 0, 0xff, 0xe0][..],
                0x5000,
                16,
            ),
        ] {
            memory.0[0x4000..0x4000 + code.len()].copy_from_slice(code);
            vmcb.initialize(3, tables.root().unwrap(), 0x1000, 0x4000, 0x10000)
                .unwrap();
            vmcb.set_permission_maps(
                core::ptr::addr_of!(IOPM) as u64,
                core::ptr::addr_of!(MSRPM) as u64,
            )
            .unwrap();
            bexos_svm_enter(
                vmcb as *mut _ as u64,
                &mut Registers::default(),
                core::ptr::addr_of!(HOST_VMCB) as u64,
            );
            assert_eq!(vmcb.exit_code(), svm::EXIT_NPF);
            assert_eq!(vmcb.exit_info().1, address);
            assert_ne!(vmcb.exit_info().0 & access, 0);
        }
        assert_eq!(&memory.0[0x6000..0x6008], &0xdecafbad_u64.to_le_bytes());
        log("svm-probe: domain read-only and execute-disable enforced\n");
        // Revoke a previously readable page, flush translations on VMRUN,
        // and require the next read to fault at the revoked guest address.
        tables.unmap(0x6000, PageSize::Small).unwrap();
        let code = [0x48, 0xa1, 0, 0x60, 0, 0, 0, 0, 0, 0];
        memory.0[0x4000..0x4000 + code.len()].copy_from_slice(&code);
        vmcb.initialize(3, tables.root().unwrap(), 0x1000, 0x4000, 0x10000)
            .unwrap();
        vmcb.set_permission_maps(
            core::ptr::addr_of!(IOPM) as u64,
            core::ptr::addr_of!(MSRPM) as u64,
        )
        .unwrap();
        bexos_svm_enter(
            vmcb as *mut _ as u64,
            &mut Registers::default(),
            core::ptr::addr_of!(HOST_VMCB) as u64,
        );
        assert_eq!(vmcb.exit_code(), svm::EXIT_NPF);
        assert_eq!(vmcb.exit_info().1, 0x6000);
        log("svm-probe: revoked domain page access rejected\n");
        // A guest that clears IF and spins without hypercalls must still lose
        // the CPU to the monitor's physical NMI watchdog.
        memory.0[0x4000..0x4003].copy_from_slice(&[0xfa, 0xeb, 0xfe]);
        vmcb.initialize(3, tables.root().unwrap(), 0x1000, 0x4000, 0x10000)
            .unwrap();
        vmcb.set_permission_maps(
            core::ptr::addr_of!(IOPM) as u64,
            core::ptr::addr_of!(MSRPM) as u64,
        )
        .unwrap();
        vmcb.intercept_nmi();
        bexos_secure_monitor::watchdog::initialize();
        bexos_svm_enter(
            vmcb as *mut _ as u64,
            &mut Registers::default(),
            core::ptr::addr_of!(HOST_VMCB) as u64,
        );
        assert_eq!(vmcb.exit_code(), svm::EXIT_NMI);
        bexos_secure_monitor::watchdog::disarm();
        bexos_secure_monitor::watchdog::service_pending();
        assert!(bexos_secure_monitor::watchdog::nmi_stack_verified());
        memory.0[0x4000..0x4003].copy_from_slice(&[0x0f, 0x01, 0xd9]);
        vmcb.set_rip(0x4000);
        bexos_svm_enter(
            vmcb as *mut _ as u64,
            &mut Registers::default(),
            core::ptr::addr_of!(HOST_VMCB) as u64,
        );
        assert_eq!(vmcb.exit_code(), svm::EXIT_VMMCALL);
        log("svm-probe: uncooperative guest preempted and resumed\n");
    }
}
