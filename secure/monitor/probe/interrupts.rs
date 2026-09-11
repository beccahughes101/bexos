//! Actual virtual IRQ delivery must respect guest IF while the physical
//! watchdog and the interrupt controller remain exclusively monitor-owned.
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
        tables
            .map(0, backing, PageSize::Large, Permissions::RAM)
            .unwrap();
        for (offset, value) in [(0x1000, 0x2007u64), (0x2000, 0x3007), (0x3000, 0x87)] {
            memory.0[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
        }
        memory.0[0x4000..0x4005].copy_from_slice(&[0xfa, 0xf4, 0xfb, 0xeb, 0xfe]); // cli; hlt; sti; spin
        memory.0[0x4100..0x4103].copy_from_slice(&[0x0f, 0x01, 0xd9]);
        // 64-bit interrupt gate for vector 64, targeting 0x4100, CS=8.
        memory.0[0x7400..0x7410]
            .copy_from_slice(&[0, 0x41, 8, 0, 0, 0x8e, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        // The CPU reads the guest GDT when entering an interrupt gate.
        memory.0[0x6000..0x6018].copy_from_slice(&[
            0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, 0, 0, 0, 0x9a, 0xaf, 0, 0xff, 0xff, 0, 0, 0, 0x92,
            0xcf, 0,
        ]);
        vmcb.initialize(4, tables.root().unwrap(), 0x1000, 0x4000, 0xf000)
            .unwrap();
        vmcb.set_idtr(0x7000, 4095).unwrap();
        vmcb.set_gdtr(0x6000, 23).unwrap();
        vmcb.set_permission_maps(
            core::ptr::addr_of!(IOPM) as u64,
            core::ptr::addr_of!(MSRPM) as u64,
        )
        .unwrap();
        let mut fabric = bexos_secure_monitor::fabric::Fabric::<1>::new().unwrap();
        assert!(fabric.write_lapic(0, 0xf0, 0x1ff, 0));
        assert!(fabric.write_lapic(0, 0x300, (1 << 18) | 64, 0));
        let mut armed = [fabric.before_entry(0, 0)];
        let mut run = [bexos_secure_monitor::run_state::RunState::default(); 1];
        vmcb.virtual_interrupt(armed[0].map(|delivery| delivery.vector));
        let mut regs = Registers::default();
        bexos_svm_enter(
            vmcb as *mut _ as u64,
            &mut regs,
            core::ptr::addr_of!(HOST_VMCB) as u64,
        );
        assert_eq!(vmcb.exit_code(), svm::EXIT_HLT);
        assert!(vmcb.virtual_interrupt_pending());
        vmcb.set_rip(0x4002);
        // Preserve a masked, pending virtual interrupt across a protected
        // CPU record restore, then require real hardware delivery after STI.
        let identity = bexos_secure_monitor::vcpu::CpuIdentity { domain: 2, cpu: 0 };
        let mut state = [0; bexos_secure_monitor::vcpu::STATE_BYTES];
        regs.snapshot(vmcb, identity, &mut state).unwrap();
        let interrupt_identity = bexos_secure_monitor::fabric::CheckpointIdentity {
            domain: 2,
            clock_epoch: 1,
        };
        let mut interrupt_state = [0; bexos_secure_monitor::fabric::Fabric::<1>::STATE_BYTES];
        fabric
            .snapshot(interrupt_identity, 0, &run, &armed, &mut interrupt_state)
            .unwrap();
        fabric = bexos_secure_monitor::fabric::Fabric::<1>::new().unwrap();
        armed = [None];
        fabric
            .restore_protected(
                interrupt_identity,
                1,
                &mut run,
                &mut armed,
                &interrupt_state,
            )
            .unwrap();
        regs = Registers::default();
        *vmcb = Vmcb::new();
        regs.restore_protected(
            vmcb,
            identity,
            bexos_secure_monitor::svm::RestorePolicy {
                asid: 4,
                npt: tables.root().unwrap(),
                iopm: core::ptr::addr_of!(IOPM) as u64,
                msrpm: core::ptr::addr_of!(MSRPM) as u64,
                mxcsr_mask: bexos_secure_monitor::vcpu::LegacyExtendedState::supported_mxcsr_mask(),
            },
            &state,
        )
        .unwrap();
        assert!(vmcb.virtual_interrupt_pending());
        assert_eq!(vmcb.rflags() & 0x200, 0);
        bexos_svm_enter(
            vmcb as *mut _ as u64,
            &mut regs,
            core::ptr::addr_of!(HOST_VMCB) as u64,
        );
        assert_eq!(vmcb.exit_code(), svm::EXIT_VMMCALL);
        assert_eq!(vmcb.rip(), 0x4100);
        assert!(!vmcb.virtual_interrupt_pending());
        assert!(fabric.delivered(0, armed[0].take().unwrap()));
        assert!(fabric.before_entry(0, 2).is_none());
        assert_eq!(fabric.read_lapic(0, 0x120, 2), Some(1));
        assert!(fabric.write_lapic(0, 0xb0, 0, 2));
        assert!(fabric.before_entry(0, 3).is_none());
        log("svm-probe: virtual interrupt masking and delivery verified\n");
        log("svm-probe: pending virtual interrupt delivered after protected CPU restore\n");
        log("svm-probe: interrupt fabric restored without duplicate delivery\n");
        // Exercise the actual entry modes needed by the normal-world BSP and
        // SIPI-started APs without disabling nested paging for either mode.
        memory.0[0x4200..0x4208].copy_from_slice(&[0xb8, 0xef, 0xbe, 0xad, 0xde, 0x0f, 0x01, 0xd9]);
        vmcb.initialize_protected(4, tables.root().unwrap(), 0x4200)
            .unwrap();
        vmcb.set_permission_maps(
            core::ptr::addr_of!(IOPM) as u64,
            core::ptr::addr_of!(MSRPM) as u64,
        )
        .unwrap();
        bexos_svm_enter(
            vmcb as *mut _ as u64,
            &mut regs,
            core::ptr::addr_of!(HOST_VMCB) as u64,
        );
        assert_eq!(vmcb.exit_code(), svm::EXIT_VMMCALL);
        assert_eq!(vmcb.rax(), 0xdeadbeef);
        memory.0[0x8000..0x8009]
            .copy_from_slice(&[0x66, 0xb8, 0xef, 0xbe, 0xad, 0xde, 0x0f, 0x01, 0xd9]);
        vmcb.initialize_startup(4, tables.root().unwrap(), 8)
            .unwrap();
        vmcb.set_permission_maps(
            core::ptr::addr_of!(IOPM) as u64,
            core::ptr::addr_of!(MSRPM) as u64,
        )
        .unwrap();
        bexos_svm_enter(
            vmcb as *mut _ as u64,
            &mut regs,
            core::ptr::addr_of!(HOST_VMCB) as u64,
        );
        assert_eq!(vmcb.exit_code(), svm::EXIT_VMMCALL);
        assert_eq!(vmcb.rax(), 0xdeadbeef);
        assert_eq!(vmcb.rip(), 6);
        log("svm-probe: protected and SIPI guest entry verified\n");
    }
}
