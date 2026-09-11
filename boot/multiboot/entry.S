/* QEMU's Multiboot1 ingress and a native Multiboot2 ingress share the loader. */
.section .multiboot,"a"
.balign 4
.long 0x1badb002, 3, -(0x1badb002 + 3)
.balign 8
mb2_header:
.long 0xe85250d6, 0, mb2_end-mb2_header, (-(0xe85250d6 + (mb2_end-mb2_header)) & 0xffffffff)
.short 0, 0
.long 8
mb2_end:
.section .text.boot,"ax"
.code32
.global _start
_start:
    cli
    cld
    mov $stack_end, %esp
    push %ebx
    push %eax
    call load_payload
    ud2
.global enter_multiboot2
enter_multiboot2:
    mov 4(%esp), %ecx
    mov 8(%esp), %ebx
    mov $0x36d76289, %eax
    jmp *%ecx
.global enter_trusty
enter_trusty:
    mov 4(%esp), %edi
    lgdt gdt_pointer
    mov %cr4, %eax
    or $0x620, %eax /* PAE, OSFXSR, OSXMMEXCPT */
    mov %eax, %cr4
    mov $pml4, %eax
    mov %eax, %cr3
    mov $0xc0000080, %ecx
    rdmsr
    or $0x900, %eax /* LME and NXE */
    wrmsr
    mov %cr0, %eax
    and $0xfffffffb, %eax
    or $0x80000002, %eax
    mov %eax, %cr0
    ljmp $8, $long_mode
.code64
long_mode:
    mov %edi, %r11d
    mov $16, %ax
    mov %ax, %ds
    mov %ax, %es
    mov %ax, %ss
    xor %edi, %edi
    xor %esi, %esi
    xor %edx, %edx
    xor %ecx, %ecx
    jmp *%r11
.balign 8
gdt:
.quad 0, 0x00af9a000000ffff, 0x00cf92000000ffff
gdt_pointer:
.short .-gdt-1
.long gdt
.section .bss,"aw",@nobits
.balign 16
.skip 32768
stack_end:
.section .note.GNU-stack,"",@progbits
