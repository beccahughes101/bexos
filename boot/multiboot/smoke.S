.section .multiboot2,"a"
.balign 8
.long 0xe85250d6, 0, 24, (-(0xe85250d6+24) & 0xffffffff)
.short 0, 0
.long 8
.section .text,"ax"
.global _start
#ifdef LONG_MODE
.code64
_start:
    mov $message, %esi
#else
.code32
_start:
    cmp $0x36d76289, %eax
    jne failed
    cmpl $16, (%ebx)
    jb failed
    mov $message, %esi
#endif
next:
    lodsb
    test %al, %al
    jz failed
    mov $0x3f8, %dx
    outb %al, %dx
    jmp next
failed:
    cli
    hlt
    jmp failed
message:
.asciz "bexos-multiboot: handoff verified\n"
