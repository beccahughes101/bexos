use core::arch::global_asm;
#[cfg(not(bexos_update_kernel))]
global_asm!(
    r#"
.section .multiboot2,"a"
.balign 8
.long 0xe85250d6
.long 0
.long 24
.long 0x17adaf12
.short 0, 0
.long 8
.section .bss.stack,"aw",@nobits
.balign 4096
.global __boot_stacks_bottom
__boot_stacks_bottom:
.skip 64 * 266240
.global __boot_stacks_top
__boot_stacks_top:
.section .bss.boot_tables,"aw",@nobits
.balign 4096
.global __x86_boot_root
__x86_boot_root: .skip 24576
.section .text.boot,"ax"
.code32
.global _start
_start:
cli
cld
mov ebp, ebx
cmp eax, 0x36d76289
jne .Lbad_boot
mov edi, offset __bss_start
mov ecx, offset __bss_end
sub ecx, edi
xor eax, eax
rep stosb
mov eax, offset __x86_boot_root
lea edx, [eax + 4096]
or edx, 3
mov [eax], edx
lea edi, [eax + 4096]
lea edx, [eax + 8192]
mov ecx, 4
.Lpdpt:
mov ebx, edx
or ebx, 3
mov [edi], ebx
add edi, 8
add edx, 4096
loop .Lpdpt
lea edi, [eax + 8192]
mov ecx, 2048
mov edx, 0x83
.Lpages:
mov [edi], edx
add edx, 0x200000
add edi, 8
loop .Lpages
mov cr3, eax
mov eax, cr4
or eax, 0x620
mov cr4, eax
mov ecx, 0xc0000080
rdmsr
or eax, 0x900
wrmsr
mov eax, cr0
and eax, 0xfffffff3
or eax, 0x80010002
mov cr0, eax
lgdt [__x86_boot_gdtr]
.byte 0xea
.long __x86_long_start
.word 8
.Lbad_boot:
hlt
jmp .Lbad_boot
.code64
__x86_long_start:
mov ax, 16
mov ds, ax
mov es, ax
mov ss, ax
lea rsp, [rip + __boot_stacks_bottom + 266240]
mov esi, ebp
xor ebp, ebp
mov edi, 0x01000000
call x86_boot_main
ud2
.balign 8
__x86_boot_gdt:
.quad 0
.quad 0x00af9a000000ffff
.quad 0x00cf92000000ffff
__x86_boot_gdtr:
.word 23
.long __x86_boot_gdt

.balign 16
.global __x86_ap_start
__x86_ap_start:
.code16
cli
cld
xor ax, ax
mov ds, ax
mov es, ax
mov ss, ax
.byte 0x0f, 0x01, 0x16
.word 0x8000 + __x86_ap_gdtr - __x86_ap_start
mov eax, cr0
or eax, 1
mov cr0, eax
.byte 0x66, 0xea
.long 0x8000 + __x86_ap_32 - __x86_ap_start
.word 8
.code32
__x86_ap_32:
mov ax, 16
mov ds, ax
mov es, ax
mov ss, ax
mov eax, [0x8ff0]
mov cr3, eax
mov eax, cr4
or eax, 0x620
mov cr4, eax
mov ecx, 0xc0000080
rdmsr
or eax, 0x900
wrmsr
mov eax, cr0
and eax, 0xfffffff3
or eax, 0x80010002
mov cr0, eax
.byte 0xea
.long 0x8000 + __x86_ap_64 - __x86_ap_start
.word 24
.code64
__x86_ap_64:
mov ebx, dword ptr [0x8ff8]
inc ebx
imul ebx, ebx, 266240
movabs rsp, offset __boot_stacks_bottom
add rsp, rbx
xor ebp, ebp
movabs rax, offset x86_secondary_entry
call rax
ud2
.balign 8
__x86_ap_gdt:
.quad 0, 0x00cf9a000000ffff, 0x00cf92000000ffff, 0x00af9a000000ffff
__x86_ap_gdtr:
.word 31
.long 0x8000 + __x86_ap_gdt - __x86_ap_start
.global __x86_ap_end
__x86_ap_end:
"#
);
#[cfg(bexos_update_kernel)]
global_asm!(
    r#"
.section .bss.stack,"aw",@nobits
.balign 4096
.global __boot_stacks_bottom
__boot_stacks_bottom: .skip 5 * 1024 * 1024
.global __boot_stacks_top
__boot_stacks_top:
.section .text.boot,"ax"
.code64
.global kernel_transplant_entry
kernel_transplant_entry:
cli
cld
lea rsp, [rip + __boot_stacks_top]
xor ebp, ebp
call kernel_transplant_main
ud2
"#
);

#[cfg(not(bexos_update_kernel))]
#[unsafe(no_mangle)]
pub extern "C" fn x86_boot_main(handoff: u64, multiboot: u64) -> ! {
    // Boot information belongs to the bootloader and is read before its memory is reclaimed.
    if multiboot != 0 && multiboot < 0x0100_0000 && multiboot % 8 == 0 {
        let len = unsafe { core::ptr::read(multiboot as *const u32) } as usize;
        if (16..=32768).contains(&len) && multiboot + len as u64 <= 0x0100_0000 {
            let bytes = unsafe { core::slice::from_raw_parts(multiboot as *const u8, len) };
            if let Some(framebuffer) = bexos_boot::framebuffer::multiboot_framebuffer(bytes) {
                let h = unsafe { &mut *(handoff as *mut bexos_boot::BootHandoff) };
                if h.magic == bexos_boot::HANDOFF_MAGIC
                    && h.version >= 3
                    && h.version <= bexos_boot::BOOT_HANDOFF_VERSION
                {
                    h.normalize_legacy_extensions();
                    h.version = bexos_boot::BOOT_HANDOFF_VERSION;
                    h.framebuffer = framebuffer;
                }
            }
        }
    }
    crate::kernel_main(handoff)
}
