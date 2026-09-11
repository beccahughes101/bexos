//! Actual two-image transfer diagnostic. The product activation path remains
//! disabled until resident fault recovery and durable commitment are connected.
use crate::{platform::Platform, secure_state};
use bexos_secure_monitor::{image::Image, monitor_image, svm::Vmcb, vcpu::Registers};
use core::arch::global_asm;
#[cfg(not(feature = "normal_world"))]
const STATE_BYTES: usize = secure_state::STATE_BYTES;
#[cfg(feature = "normal_world")]
const STATE_BYTES: usize = crate::domain_state::STATE_BYTES;
const HANDOFF_BYTES: usize = STATE_BYTES
    + 6 * 8
    + bexos_trusty_boot::avb::STATE_BYTES
    + if cfg!(feature = "firmware_recovery_probe") {
        320
    } else {
        0
    };

#[unsafe(link_section = ".monitor.abi")]
#[used]
static ABI: [u64; 8] = monitor_image::descriptor(HANDOFF_BYTES);
#[unsafe(link_section = ".resident.transfer")]
static mut RECORD: [u8; STATE_BYTES] = [0; STATE_BYTES];
#[unsafe(link_section = ".resident.transfer_control")]
#[used]
static mut CONTROL: [u64; 6] = [0; 6];
#[unsafe(link_section = ".resident.client")]
#[used]
static mut CLIENT: [u8; bexos_trusty_boot::avb::STATE_BYTES] =
    [0; bexos_trusty_boot::avb::STATE_BYTES];
const CONTROL_MAGIC: u64 = u64::from_le_bytes(*b"BEXMC001");

global_asm!(
    r#"
.section .resident.stack,"aw",@nobits
.balign 4096
monitor_transition_stack:
.skip 1048576
monitor_transition_stack_end:
.section .resident.text,"ax"
.global monitor_rollback_bank
monitor_rollback_bank:
    cli
    clgi
    lea rdx, [rip + {control}]
    mov rsi, [rdx + 8]
    mov rdi, 0x60000000
    cmp rsi, 0x04000000
    je 3f
    mov rdi, 0x04000000
3:
    mov [rdx + 8], rdi
    mov qword ptr [rdx + 40], 1
    jmp monitor_switch_bank
.global monitor_switch_bank
monitor_switch_bank:
    cli
    clgi
    lea rsp, [rip + monitor_transition_stack_end]
    and rsp, -16
    // rdi is the inactive bank's physical base. Replace only the image window.
    lea rdx, [rip + PD]
    mov rcx, 32
1:
    mov rax, rdi
    or rax, 0x83
    mov [rdx + rcx*8], rax
    add rdi, 0x200000
    inc rcx
    cmp rcx, 192
    jne 1b
    // Keep a private alias for retiring the previous physical image.
    mov rcx, 1024
    mov rdi, rsi
2:
    mov rax, rdi
    or rax, 0x83
    mov [rdx + rcx*8], rax
    add rdi, 0x200000
    inc rcx
    cmp rcx, 1184
    jne 2b
    mov rax, cr3
    mov cr3, rax
    mov rax, 0x04001000
    jmp rax
.section .text.resume,"ax"
.global monitor_resume_entry
monitor_resume_entry:
    cli
    cld
    // Aggregate restoration nests page-aligned CPU and transport records.
    // Keep it on the 1 MiB resident stack, outside both reclaimable images.
    lea rsp, [rip + monitor_transition_stack_end]
    and rsp, -16
    call monitor_resume
    ud2
"#,
    control = sym CONTROL,
);
unsafe extern "C" {
    fn monitor_switch_bank(physical: u64, previous: u64) -> !;
    fn monitor_rollback_bank() -> !;
}

#[cfg(all(
    not(feature = "monitor_candidate"),
    not(feature = "firmware_recovery_probe")
))]
pub unsafe fn transfer(
    #[cfg(feature = "normal_world")] normal: &mut crate::normal::Normal,
    vmcb: &mut Vmcb,
    regs: &mut Registers,
    platform: &mut Platform<1>,
) -> ! {
    let bytes = include_bytes!(env!("MONITOR_CANDIDATE"));
    unsafe {
        transfer_image(
            bytes,
            #[cfg(feature = "normal_world")]
            normal,
            vmcb,
            regs,
            platform,
        )
    }
}

pub unsafe fn transfer_image(
    bytes: &[u8],
    #[cfg(feature = "normal_world")] normal: &mut crate::normal::Normal,
    vmcb: &mut Vmcb,
    regs: &mut Registers,
    platform: &mut Platform<1>,
) -> ! {
    let control = unsafe { core::ptr::read_volatile(core::ptr::addr_of!(CONTROL)) };
    let (previous, epoch) = if control == [0; 6] {
        (monitor_image::IMAGE_BASE, 1)
    } else {
        assert_eq!(control[0], CONTROL_MAGIC);
        (control[1] as usize, control[2].checked_add(1).unwrap())
    };
    let (next, destination) = match previous {
        monitor_image::IMAGE_BASE => (monitor_image::INACTIVE_BANK, monitor_image::INACTIVE_BANK),
        monitor_image::INACTIVE_BANK => (monitor_image::IMAGE_BASE, 0x80000000),
        _ => panic!("invalid resident image bank"),
    };
    let image = Image::parse(bytes, monitor_image::RESIDENT_END).unwrap();
    monitor_image::validate(&image, HANDOFF_BYTES).unwrap();
    image
        .require_resident_code(monitor_image::IMAGE_END, unsafe {
            core::slice::from_raw_parts(monitor_image::IMAGE_END as *const u8, 4096)
        })
        .unwrap();
    // Preparation writes only the inactive bank; retained hardware is omitted.
    let inactive = unsafe {
        core::slice::from_raw_parts_mut(destination as *mut u8, monitor_image::IMAGE_BYTES)
    };
    let preparation_start = unsafe { bexos_secure_monitor::clock::now_ns() };
    for offset in (0..monitor_image::IMAGE_BYTES).step_by(1024 * 1024) {
        image
            .load_replaceable_chunk(
                monitor_image::IMAGE_BASE,
                inactive,
                monitor_image::IMAGE_END..monitor_image::RESIDENT_END,
                offset,
                (1024 * 1024).min(monitor_image::IMAGE_BYTES - offset),
            )
            .unwrap();
        unsafe {
            #[cfg(feature = "normal_world")]
            crate::schedule::normal_turn(normal);
            crate::secure_step(vmcb, regs, platform);
            assert!(
                bexos_secure_monitor::clock::now_ns()
                    .checked_sub(preparation_start)
                    .unwrap()
                    < 30_000_000_000
            );
        }
    }
    crate::log("monitor-runtime: candidate prepared while retained guests continued running\n");
    let mut avb = bexos_trusty_boot::avb::Avb::connect(unsafe {
        crate::transport::Boot::new(|| {
            #[cfg(feature = "normal_world")]
            crate::schedule::normal_turn(normal);
            crate::secure_step(vmcb, regs, platform);
        })
    })
    .unwrap();
    let floor = avb.read_rollback(0).unwrap();
    avb.begin_read_rollback(0).unwrap();
    let (owner, client) = avb.suspend();
    unsafe {
        core::ptr::write_volatile(core::ptr::addr_of_mut!(CLIENT), client);
        owner.detach();
    }
    let cutover_start = unsafe { bexos_secure_monitor::clock::now_ns() };
    #[cfg(not(feature = "normal_world"))]
    unsafe {
        secure_state::snapshot(
            epoch,
            vmcb,
            regs,
            platform,
            &mut *core::ptr::addr_of_mut!(RECORD),
        )
    }
    .unwrap();
    #[cfg(feature = "normal_world")]
    unsafe {
        crate::domain_state::snapshot(
            epoch,
            normal,
            vmcb,
            regs,
            platform,
            &mut *core::ptr::addr_of_mut!(RECORD),
        )
    }
    .unwrap();
    crate::log("monitor-runtime: protected snapshot duration ns=\n");
    crate::hex(
        unsafe { bexos_secure_monitor::clock::now_ns() }
            .checked_sub(cutover_start)
            .unwrap(),
    );
    unsafe {
        core::ptr::write_volatile(
            core::ptr::addr_of_mut!(CONTROL),
            [CONTROL_MAGIC, next as u64, epoch, cutover_start, floor, 0],
        )
    };
    crate::log("monitor-runtime: separate candidate loaded; entering resident transition\n");
    unsafe {
        let elapsed = bexos_secure_monitor::clock::now_ns()
            .checked_sub(cutover_start)
            .unwrap();
        let Some(remaining) = 150_000_000u64
            .checked_sub(elapsed)
            .filter(|remaining| *remaining != 0)
        else {
            monitor_rollback_bank();
        };
        bexos_secure_monitor::watchdog::arm_recovery(
            monitor_rollback_bank as *const () as u64,
            remaining,
        );
        monitor_switch_bank(next as u64, previous as u64)
    }
}

#[unsafe(no_mangle)]
extern "C" fn monitor_resume() -> ! {
    unsafe {
        let control = core::ptr::read_volatile(core::ptr::addr_of!(CONTROL));
        assert_eq!(control[0], CONTROL_MAGIC);
        let epoch = control[2];
        let rolled_back = control[5] == 1;
        assert!(control[5] <= 1);
        #[cfg(not(feature = "monitor_candidate"))]
        assert!(
            rolled_back,
            "transfer entered the old image without resident recovery"
        );
        #[cfg(feature = "monitor_entry_fault")]
        if !rolled_back {
            core::arch::asm!("ud2", options(noreturn));
        }
        #[cfg(feature = "monitor_entry_hang")]
        if !rolled_back {
            core::arch::asm!("stgi", "2: jmp 2b", options(noreturn));
        }

        let vmcb = &mut *core::ptr::addr_of_mut!(crate::VCPU);
        #[cfg(not(feature = "normal_world"))]
        let mut regs = Registers::default();
        #[cfg(not(feature = "normal_world"))]
        let mut platform = Platform::<1>::new(
            crate::memory::DomainMemory {
                base: crate::BANK,
                length: crate::BANK_SIZE,
            },
            true,
        );
        #[cfg(not(feature = "normal_world"))]
        secure_state::restore_protected(
            epoch,
            &*core::ptr::addr_of!(RECORD),
            vmcb,
            &mut regs,
            &mut platform,
        )
        .unwrap();
        #[cfg(feature = "normal_world")]
        let (mut normal, mut regs, mut platform) =
            crate::domain_state::prepare_protected(epoch, &*core::ptr::addr_of!(RECORD))
                .unwrap()
                .install_owners(vmcb);
        crate::log(if rolled_back {
            "monitor-runtime: old monitor resumed protected Trusty state through resident recovery\n"
        } else {
            "monitor-runtime: distinct candidate monitor image resumed retained Trusty\n"
        });
        crate::log("monitor-runtime: snapshot-through-owner-import ns=\n");
        crate::hex(
            bexos_secure_monitor::clock::now_ns()
                .checked_sub(control[3])
                .unwrap(),
        );
        bexos_secure_monitor::watchdog::service_pending();
        #[cfg(feature = "normal_world")]
        for _ in 0..4 {
            normal.step();
        }
        let client = core::ptr::read_volatile(core::ptr::addr_of!(CLIENT));
        let mut avb = bexos_trusty_boot::avb::Avb::restore_protected(
            crate::transport::Boot::resume(|| crate::secure_step(vmcb, &mut regs, &mut platform)),
            &client,
        )
        .unwrap();
        let floor = avb.finish_read_rollback().unwrap();
        assert_eq!(floor, control[4]);
        crate::log(
            "monitor-runtime: retained secure client completed its in-flight request after distinct monitor entry\n",
        );
        avb.close().unwrap();
        drop(avb);
        let readiness_ns = bexos_secure_monitor::clock::now_ns()
            .checked_sub(control[3])
            .unwrap();
        crate::log("monitor-runtime: measured snapshot-to-secure-readiness ns=\n");
        crate::hex(readiness_ns);
        #[cfg(not(feature = "firmware_recovery_probe"))]
        assert!(
            rolled_back || readiness_ns <= 150_000_000,
            "live readiness deadline exceeded"
        );
        if rolled_back {
            for chunk in
                core::slice::from_raw_parts_mut(0x80000000 as *mut u8, monitor_image::IMAGE_BYTES)
                    .chunks_mut(1024 * 1024)
            {
                chunk.fill(0);
                #[cfg(feature = "normal_world")]
                crate::schedule::normal_turn(&mut normal);
                crate::secure_step(vmcb, &mut regs, &mut platform);
            }
            crate::log(
                "monitor-runtime: resident recovery resumed old monitor and retained in-flight secure client\n",
            );
            loop {
                #[cfg(feature = "normal_world")]
                crate::schedule::normal_turn(&mut normal);
                crate::secure_step(vmcb, &mut regs, &mut platform);
            }
        }
        #[cfg(all(feature = "firmware_recovery_probe", feature = "monitor_candidate"))]
        crate::firmware_recovery::ready(&mut crate::transport::Boot::new(|| {
            crate::secure_step(vmcb, &mut regs, &mut platform)
        }));
        // Diagnostic only: no product commitment is claimed. Poison and reuse
        // the entire old image through its resident mapping, then require IPC.
        let old =
            core::slice::from_raw_parts_mut(0x80000000 as *mut u8, monitor_image::IMAGE_BYTES);
        for chunk in old.chunks_mut(1024 * 1024) {
            chunk.fill(0xa5);
            assert!(chunk.iter().all(|byte| *byte == 0xa5));
            #[cfg(feature = "normal_world")]
            crate::schedule::normal_turn(&mut normal);
            crate::secure_step(vmcb, &mut regs, &mut platform);
        }
        let mut avb = bexos_trusty_boot::avb::Avb::connect(crate::transport::Boot::new(|| {
            crate::secure_step(vmcb, &mut regs, &mut platform)
        }))
        .unwrap();
        assert_eq!(avb.read_rollback(0).unwrap(), floor);
        avb.close().unwrap();
        drop(avb);
        crate::log(
            "monitor-runtime: real Trusty IPC continued after distinct monitor entry and old-image reuse\n",
        );
        if epoch > 1 {
            crate::log(
                "monitor-runtime: repeated distinct monitor replacement reused both image banks\n",
            );
        }
        #[cfg(feature = "monitor_successor")]
        transfer_image(
            include_bytes!(env!("MONITOR_CANDIDATE")),
            #[cfg(feature = "normal_world")]
            &mut normal,
            vmcb,
            &mut regs,
            &mut platform,
        );
        loop {
            #[cfg(feature = "normal_world")]
            crate::schedule::normal_turn(&mut normal);
            crate::secure_step(vmcb, &mut regs, &mut platform);
        }
    }
}
