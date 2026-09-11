//! Root-owned NMI watchdog. Guest interrupt masking cannot prevent the monitor
//! regaining control. This x86 backend owns its GDT/IDT and local APIC timer.
use core::{
    arch::{asm, global_asm},
    sync::atomic::{AtomicU64, Ordering},
};
const APIC: u64 = 0xfee00000;

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct Gate {
    low: u16,
    selector: u16,
    ist: u8,
    flags: u8,
    middle: u16,
    high: u32,
    reserved: u32,
}
impl Gate {
    const EMPTY: Self = Self {
        low: 0,
        selector: 8,
        ist: 0,
        flags: 0x8e,
        middle: 0,
        high: 0,
        reserved: 0,
    };
    fn new(address: u64, ist: u8) -> Self {
        Self {
            ist,
            low: address as u16,
            middle: (address >> 16) as u16,
            high: (address >> 32) as u32,
            ..Self::EMPTY
        }
    }
}
#[repr(C, packed)]
struct Descriptor {
    limit: u16,
    base: u64,
}
#[unsafe(link_section = ".resident.gdt")]
static mut GDT: [u64; 5] = [0, 0x00af9a000000ffff, 0x00cf92000000ffff, 0, 0];
#[unsafe(link_section = ".resident.tss")]
static mut TSS: [u8; 104] = [0; 104];
#[unsafe(link_section = ".resident.idt")]
static mut IDT: [Gate; 256] = [Gate::EMPTY; 256];
#[unsafe(no_mangle)]
#[unsafe(link_section = ".resident.ticks")]
static BEXOS_MONITOR_NMI_TICKS: AtomicU64 = AtomicU64::new(0);
#[unsafe(no_mangle)]
#[unsafe(link_section = ".resident.ticks")]
static BEXOS_MONITOR_NMI_RSP: AtomicU64 = AtomicU64::new(0);
#[unsafe(no_mangle)]
#[unsafe(link_section = ".resident.ticks")]
static BEXOS_MONITOR_RECOVERY_ENTRY: AtomicU64 = AtomicU64::new(0);
#[unsafe(no_mangle)]
#[unsafe(link_section = ".resident.ticks")]
static BEXOS_MONITOR_RECOVERY_DEADLINE: AtomicU64 = AtomicU64::new(0);

global_asm!(
    r#"
.section .resident.exceptions,"aw",@nobits
.balign 4096
.global bexos_monitor_nmi_stack
bexos_monitor_nmi_stack:
.skip 8192
.global bexos_monitor_nmi_stack_end
bexos_monitor_nmi_stack_end:
.skip 8192
.global bexos_monitor_fault_stack_end
bexos_monitor_fault_stack_end:
.skip 8192
.global bexos_monitor_double_fault_stack_end
bexos_monitor_double_fault_stack_end:
.section .resident.text,"ax"
.global bexos_monitor_nmi
bexos_monitor_nmi:
    mov qword ptr [rip + BEXOS_MONITOR_NMI_RSP], rsp
    lock inc qword ptr [rip + BEXOS_MONITOR_NMI_TICKS]
    push rax
    push rdx
    mov rax, qword ptr [rip + BEXOS_MONITOR_RECOVERY_ENTRY]
    test rax, rax
    jz 8f
    mov edx, 0xfed000f0
    mov rdx, [rdx]
    cmp rdx, qword ptr [rip + BEXOS_MONITOR_RECOVERY_DEADLINE]
    jb 8f
    mov qword ptr [rip + BEXOS_MONITOR_RECOVERY_ENTRY], 0
    // IST supplies a complete frame. IRET releases NMI blocking, then the
    // resident recovery entry immediately installs its own protected stack.
    mov [rsp + 16], rax
8:
    pop rdx
    pop rax
    iretq
.global bexos_monitor_root_fault
bexos_monitor_root_fault:
    cli
    mov rax, qword ptr [rip + BEXOS_MONITOR_RECOVERY_ENTRY]
    test rax, rax
    jz 7f
    mov qword ptr [rip + BEXOS_MONITOR_RECOVERY_ENTRY], 0
    jmp rax
7:
    mov rsi, rsp
    mov dx, 0x3f8
    lea rbx, [rip + bexos_monitor_fault_message]
2:
    mov al, [rbx]
    test al, al
    jz 3f
    out dx, al
    inc rbx
    jmp 2b
3:
    mov rdi, 4
4:
    mov rbx, [rsi]
    mov rcx, 16
5:
    rol rbx, 4
    mov al, bl
    and al, 15
    cmp al, 10
    jb 6f
    add al, 39
6:
    add al, 48
    out dx, al
    loop 5b
    mov al, 10
    out dx, al
    add rsi, 8
    dec rdi
    jnz 4b
1:  hlt
    jmp 1b
bexos_monitor_fault_message:
    .asciz "monitor-runtime: root fault frame\n"
"#
);
unsafe extern "C" {
    fn bexos_monitor_nmi();
    fn bexos_monitor_root_fault();
    static bexos_monitor_nmi_stack: u8;
    static bexos_monitor_nmi_stack_end: u8;
    static bexos_monitor_fault_stack_end: u8;
    static bexos_monitor_double_fault_stack_end: u8;
}

/// # Safety
/// BSP-only bootstrap with interrupts disabled. No other CPU may use these
/// tables while being initialized. Host memory and APIC MMIO must be identity
/// mapped and private from every domain. Guest NMI interception is mandatory.
pub unsafe fn initialize() {
    unsafe {
        let table = &mut *core::ptr::addr_of_mut!(TSS);
        table.fill(0);
        for (offset, stack) in [
            (36, core::ptr::addr_of!(bexos_monitor_nmi_stack_end) as u64),
            (
                44,
                core::ptr::addr_of!(bexos_monitor_fault_stack_end) as u64,
            ),
            (
                52,
                core::ptr::addr_of!(bexos_monitor_double_fault_stack_end) as u64,
            ),
        ] {
            table[offset..offset + 8].copy_from_slice(&stack.to_le_bytes());
        }
        table[102..104].copy_from_slice(&104u16.to_le_bytes());
        let base = core::ptr::addr_of!(TSS) as u64;
        let entries = &mut *core::ptr::addr_of_mut!(GDT);
        entries[3] = 103 | ((base & 0xff_ffff) << 16) | (0x89 << 40) | ((base & 0xff00_0000) << 32);
        entries[4] = base >> 32;
        let gdt = Descriptor {
            limit: (core::mem::size_of_val(entries) - 1) as u16,
            base: entries.as_ptr() as u64,
        };
        asm!("lgdt [{}]", in(reg) &gdt, options(readonly, nostack));
        asm!("push 8", "lea rax, [rip + 2f]", "push rax", "retfq", "2:",
            "mov ax, 16", "mov ds, ax", "mov es, ax", "mov ss, ax", out("rax") _);
        asm!("mov ax, 24", "ltr ax", out("rax") _, options(nostack));
        let idt = &mut *core::ptr::addr_of_mut!(IDT);
        idt.fill(Gate::new(bexos_monitor_root_fault as *const () as u64, 2));
        idt[2] = Gate::new(bexos_monitor_nmi as *const () as u64, 1);
        idt[8] = Gate::new(bexos_monitor_root_fault as *const () as u64, 3);
        let descriptor = Descriptor {
            limit: (core::mem::size_of_val(idt) - 1) as u16,
            base: idt.as_ptr() as u64,
        };
        asm!("lidt [{}]", in(reg) &descriptor, options(readonly, nostack));
        let (mut lo, hi): (u32, u32);
        asm!("rdmsr", in("ecx") 0x1bu32, out("eax") lo, out("edx") hi, options(nomem, nostack));
        assert_eq!(((hi as u64) << 32 | lo as u64) & 0xfffff000, APIC);
        assert_eq!(lo & (1 << 10), 0, "x2APIC root requires a separate backend");
        lo |= 1 << 11;
        asm!("wrmsr", in("ecx") 0x1bu32, in("eax") lo, in("edx") hi, options(nomem, nostack));
        write(0xf0, 0x1ff);
        write(0x3e0, 3); // Divide the local APIC input clock by 16.
        write(0x320, (1 << 17) | (4 << 8)); // Periodic timer, NMI delivery.
        write(0x380, 100_000);
        asm!("stgi", options(nomem, nostack));
    }
}
/// Restore GIF only after entry.S has restored all root-owned CPU state. This
/// consumes a physical intercepted NMI using the root handler before re-entry.
/// # Safety
/// The private root interrupt tables above must be installed on this CPU.
pub unsafe fn service_pending() {
    unsafe {
        asm!("stgi", options(nomem, nostack));
    }
}
pub fn ticks() -> u64 {
    BEXOS_MONITOR_NMI_TICKS.load(Ordering::Acquire)
}
pub fn nmi_stack_verified() -> bool {
    let rsp = BEXOS_MONITOR_NMI_RSP.load(Ordering::Acquire);
    (core::ptr::addr_of!(bexos_monitor_nmi_stack) as u64
        ..core::ptr::addr_of!(bexos_monitor_nmi_stack_end) as u64)
        .contains(&rsp)
}
/// # Safety
/// The callback and its stack are resident. The saved state is authoritative
/// and no guest may execute before disarm_recovery; otherwise rollback could
/// rewind CPU state relative to already-mutated guest memory and devices.
pub unsafe fn arm_recovery(entry: u64, duration_ns: u64) {
    assert!((0x18000000..0x18001000).contains(&entry));
    let deadline = unsafe { crate::clock::deadline_ticks(duration_ns) }.unwrap();
    BEXOS_MONITOR_RECOVERY_DEADLINE.store(deadline, Ordering::Release);
    BEXOS_MONITOR_RECOVERY_ENTRY.store(entry, Ordering::Release);
}
pub fn disarm_recovery() {
    if BEXOS_MONITOR_RECOVERY_ENTRY.load(Ordering::Acquire) != 0 {
        BEXOS_MONITOR_RECOVERY_ENTRY.store(0, Ordering::Release);
    }
}
/// # Safety
/// Caller owns this CPU's timer and its identity-mapped APIC registers.
pub unsafe fn disarm() {
    unsafe {
        write(0x320, 1 << 16);
        write(0x380, 0);
    }
}
unsafe fn write(offset: u64, value: u32) {
    unsafe {
        core::ptr::write_volatile((APIC + offset) as *mut u32, value);
    }
}
