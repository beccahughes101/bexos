//! Exercise real interleaved guest entries; host assertions cannot substitute
//! for preservation of guest registers across VMRUN and unrelated host work.
use super::*;

global_asm!(
    r#"
.section .rodata
.global context_guest_start
.global context_guest_end
context_guest_start:
    inc rbx
    inc rcx
    inc rdx
    inc rsi
    inc rdi
    inc rbp
    inc r8
    inc r9
    inc r10
    inc r11
    inc r12
    inc r13
    inc r14
    inc r15
    paddq xmm0, xmm1
    vmmcall
    ud2
context_guest_end:
"#
);
unsafe extern "C" {
    static context_guest_start: u8;
    static context_guest_end: u8;
}

static mut SECOND_GUEST: GuestMemory = GuestMemory([0; 2097152]);
static mut SECOND_NPT: DomainMap = DomainMap::new();
static mut SECOND_VMCB: Vmcb = Vmcb::new();

pub unsafe fn verify() {
    unsafe {
        let second_memory = &mut *core::ptr::addr_of_mut!(SECOND_GUEST);
        let second_map = &mut *core::ptr::addr_of_mut!(SECOND_NPT);
        second_map
            .map_probe_memory(second_memory as *mut _ as u64)
            .unwrap();
        let code = core::slice::from_raw_parts(
            core::ptr::addr_of!(context_guest_start),
            core::ptr::addr_of!(context_guest_end) as usize
                - core::ptr::addr_of!(context_guest_start) as usize,
        );
        let mut registers = [Registers::default(), Registers::default()];
        let mut guests = [
            &mut *core::ptr::addr_of_mut!(VMCB),
            &mut *core::ptr::addr_of_mut!(SECOND_VMCB),
        ];
        let mut memories = [&mut *core::ptr::addr_of_mut!(GUEST), second_memory];
        let roots = [core::ptr::addr_of!(NPT) as u64, second_map as *mut _ as u64];
        for index in 0..2 {
            let memory = &mut memories[index];
            for (offset, value) in [(0x1000, 0x2007u64), (0x2000, 0x3007), (0x3000, 0x87)] {
                memory.0[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
            }
            memory.0[0x5000..0x5000 + code.len()].copy_from_slice(code);
            let vmcb = &mut guests[index];
            vmcb.initialize(index as u32 + 1, roots[index], 0x1000, 0x5000, 0x10000)
                .unwrap();
            vmcb.set_permission_maps(
                core::ptr::addr_of!(IOPM) as u64,
                core::ptr::addr_of!(MSRPM) as u64,
            )
            .unwrap();
            registers[index].extended.set_xmm(0, index as u128 * 10000);
            registers[index].extended.set_xmm(1, 1);
        }
        for round in 1..=128u64 {
            for index in 0..2 {
                let vmcb = &mut guests[index];
                vmcb.set_rip(0x5000);
                bexos_svm_enter(
                    *vmcb as *mut _ as u64,
                    &mut registers[index],
                    core::ptr::addr_of!(HOST_VMCB) as u64,
                );
                assert_eq!(vmcb.exit_code(), svm::EXIT_VMMCALL);
                let r = &registers[index];
                assert_eq!(
                    [
                        r.rbx, r.rcx, r.rdx, r.rsi, r.rdi, r.rbp, r.r8, r.r9, r.r10, r.r11, r.r12,
                        r.r13, r.r14, r.r15
                    ],
                    [round; 14]
                );
                assert_eq!(
                    r.extended.xmm(0),
                    Some(index as u128 * 10000 + round as u128)
                );
                if round % 16 == 0 {
                    use bexos_secure_monitor::{
                        svm::RestorePolicy,
                        vcpu::{CpuIdentity, STATE_BYTES},
                    };
                    let identity = CpuIdentity {
                        domain: index as u32 + 1,
                        cpu: 0,
                    };
                    let mut state = [0; STATE_BYTES];
                    // Finish this intercepted call before snapshotting. The
                    // next actual guest entry must continue the same counters.
                    vmcb.set_rip(0x5000);
                    registers[index]
                        .snapshot(vmcb, identity, &mut state)
                        .unwrap();
                    registers[index] = Registers::default();
                    **vmcb = Vmcb::new();
                    registers[index]
                        .restore_protected(
                            vmcb,
                            identity,
                            RestorePolicy {
                                asid: index as u32 + 1,
                                npt: roots[index],
                                iopm: core::ptr::addr_of!(IOPM) as u64,
                                msrpm: core::ptr::addr_of!(MSRPM) as u64,
                                mxcsr_mask: bexos_secure_monitor::vcpu::LegacyExtendedState::supported_mxcsr_mask(),
                            },
                            &state,
                        )
                        .unwrap();
                }
                // Distinguishable host work between every pair of guest entries.
                core::hint::black_box([round.wrapping_mul(0x123456789); 16]);
            }
        }
        log("svm-probe: protected CPU records resumed interleaved guest execution\n");
        verify_registration(&mut guests, &mut memories, &mut registers);
    }
}

unsafe fn verify_registration(
    guests: &mut [&mut Vmcb; 2],
    memories: &mut [&mut GuestMemory; 2],
    registers: &mut [Registers; 2],
) {
    use bexos_secure_monitor::shared::{Caller, DomainId, RamWindow, Registry};
    use bexos_secure_monitor_abi::{Request, SHARED_READ, Status};
    let caller = [
        Caller {
            domain: DomainId(1),
            may_share: true,
        },
        Caller {
            domain: DomainId(2),
            may_share: false,
        },
    ];
    let mut registry = Registry::<1, 2>::new([RamWindow {
        owner: caller[0].domain,
        guest_start: 0x180000,
        host_start: memories[0] as *mut _ as u64 + 0x180000,
        length: 4096,
    }])
    .unwrap();
    for memory in memories {
        memory.0[0x6000..0x6006].copy_from_slice(&[0x0f, 0x01, 0xd9, 0x0f, 0x01, 0xd9]);
    }
    let request = Request::Register {
        address: 0x180000,
        length: 4096,
        access: SHARED_READ,
    }
    .encode();
    let mut first_handle = 0;
    for (domain, request, expected) in [
        (1, request, Status::AccessDenied),
        (
            0,
            Request::Register {
                address: 0x200000,
                length: 4096,
                access: SHARED_READ,
            }
            .encode(),
            Status::AccessDenied,
        ),
        (0, request, Status::Ok),
        (0, Request::Unregister { handle: 1 }.encode(), Status::Ok),
        (
            0,
            Request::Unregister { handle: 1 }.encode(),
            Status::InvalidHandle,
        ),
        (0, request, Status::Ok),
    ] {
        let vmcb = &mut guests[domain];
        let r = &mut registers[domain];
        vmcb.set_rip(0x6000);
        vmcb.set_rax(request[0]);
        [r.rbx, r.rcx, r.rdx, r.rsi, r.rdi, r.r8, r.r9] = request[1..].try_into().unwrap();
        unsafe {
            bexos_svm_enter(
                *vmcb as *mut _ as u64,
                r,
                core::ptr::addr_of!(HOST_VMCB) as u64,
            );
        }
        assert_eq!(vmcb.exit_code(), svm::EXIT_VMMCALL);
        let result = registry.dispatch(
            caller[domain],
            [vmcb.rax(), r.rbx, r.rcx, r.rdx, r.rsi, r.rdi, r.r8, r.r9],
        );
        assert_eq!(result[0], expected as i64 as u64);
        if result[1] != 0 {
            assert_ne!(result[1], first_handle);
            first_handle = result[1];
        }
        vmcb.set_rax(result[0]);
        [r.rbx, r.rcx, r.rdx, r.rsi, r.rdi, r.r8, r.r9] = result[1..].try_into().unwrap();
        vmcb.set_rip(0x6003);
        unsafe {
            bexos_svm_enter(
                *vmcb as *mut _ as u64,
                r,
                core::ptr::addr_of!(HOST_VMCB) as u64,
            );
        }
        assert_eq!(vmcb.exit_code(), svm::EXIT_VMMCALL);
        assert_eq!(
            [vmcb.rax(), r.rbx, r.rcx, r.rdx, r.rsi, r.rdi, r.r8, r.r9],
            result
        );
    }
    log("svm-probe: hypercall capability ownership and stale-handle rejection verified\n");
}
