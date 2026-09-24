core::arch::global_asm!(
    r#"
.section .text,"ax"
.code64
.altmacro
.macro vector_entry number
.global __x86_vector_\number
__x86_vector_\number:
.if (\number != 8) && (\number != 10) && (\number != 11) && (\number != 12) && (\number != 13) && (\number != 14) && (\number != 17) && (\number != 21) && (\number != 29) && (\number != 30)
push 0
.endif
push \number
jmp __x86_save_context
.endm
.set vector_number, 0
.rept 256
vector_entry %vector_number
.set vector_number, vector_number + 1
.endr
.noaltmacro
.global __x86_syscall_entry
__x86_syscall_entry:
swapgs
mov qword ptr gs:[8], rsp
mov rsp, qword ptr gs:[0]
push 0x1b
push qword ptr gs:[8]
push r11
push 0x23
push rcx
push 0
push 129
jmp __x86_save_context
__x86_save_context:
cld
sub rsp, 824
mov [rsp + 0], rdi
mov [rsp + 8], rsi
mov [rsp + 16], rdx
mov [rsp + 24], r10
mov [rsp + 32], r8
mov [rsp + 40], r9
mov [rsp + 48], r12
mov [rsp + 56], r13
mov [rsp + 64], r14
mov [rsp + 72], r15
mov [rsp + 80], rax
mov [rsp + 88], rcx
mov [rsp + 96], rbx
mov [rsp + 104], rbp
mov [rsp + 112], r11
xor eax, eax
mov [rsp + 120], rax
mov [rsp + 128], rax
mov [rsp + 136], rax
mov [rsp + 144], rax
mov [rsp + 152], rax
mov [rsp + 160], rax
mov [rsp + 168], rax
mov [rsp + 176], rax
mov [rsp + 184], rax
mov [rsp + 192], rax
mov [rsp + 200], rax
mov [rsp + 208], rax
mov [rsp + 216], rax
mov [rsp + 224], rax
mov [rsp + 232], rax
mov [rsp + 240], rax
mov rax, [rsp + 824]
cmp rax, 129
jne 4f
mov ecx, 0xc0000102
jmp 5f
4:
mov ecx, 0xc0000101
5:
rdmsr
shl rdx, 32
or rax, rdx
mov [rsp + 120], rax
mov rax, [rsp + 848]
mov [rsp + 240], rax
mov rax, [rsp + 864]
mov [rsp + 248], rax
mov rax, [rsp + 840]
mov [rsp + 256], rax
mov rax, [rsp + 856]
mov [rsp + 264], rax
fxsave64 [rsp + 272]
xor eax, eax
mov [rsp + 784], rax
mov [rsp + 792], rax
mov ecx, 0xc0000100
rdmsr
shl rdx, 32
or rax, rdx
mov [rsp + 800], rax
mov qword ptr [rsp + 808], 2
mov rdi, rsp
mov rsi, [rsp + 824]
mov rdx, [rsp + 832]
call x86_handle_trap
.global __x86_restore_context
__x86_restore_context:
mov rax, [rsp + 256]
mov [rsp + 840], rax
mov rax, [rsp + 248]
mov [rsp + 864], rax
mov rax, [rsp + 264]
mov [rsp + 856], rax
mov rax, [rsp + 240]
mov [rsp + 848], rax
and eax, 3
jz 2f
mov qword ptr [rsp + 872], 0x1b
jmp 3f
2:
mov qword ptr [rsp + 872], 0x10
3:
mov rax, [rsp + 800]
mov rdx, rax
shr rdx, 32
mov ecx, 0xc0000100
wrmsr
mov rax, [rsp + 120]
mov rdx, rax
shr rdx, 32
mov rax, [rsp + 824]
cmp rax, 129
jne 4f
mov ecx, 0xc0000102
jmp 5f
4:
mov ecx, 0xc0000101
5:
mov rax, [rsp + 120]
wrmsr
fxrstor64 [rsp + 272]
mov rdi, [rsp + 0]
mov rsi, [rsp + 8]
mov rdx, [rsp + 16]
mov r10, [rsp + 24]
mov r8, [rsp + 32]
mov r9, [rsp + 40]
mov r12, [rsp + 48]
mov r13, [rsp + 56]
mov r14, [rsp + 64]
mov r15, [rsp + 72]
mov rax, [rsp + 80]
mov rcx, [rsp + 88]
mov rbx, [rsp + 96]
mov rbp, [rsp + 104]
mov r11, [rsp + 112]
cmp qword ptr [rsp + 824], 129
jne 4f
swapgs
4:
add rsp, 840
iretq
.global x86_enter_context
x86_enter_context:
cli
mov rsi, rdi
sub rsp, 896
and rsp, -16
mov rdi, rsp
mov ecx, 102
cld
rep movsq
mov qword ptr [rsp + 824], 0
jmp __x86_restore_context
.section .rodata,"a"
.global __x86_vectors
__x86_vectors:
.altmacro
.macro vector_pointer number
.quad __x86_vector_\number
.endm
.set vector_number, 0
.rept 256
vector_pointer %vector_number
.set vector_number, vector_number + 1
.endr
.noaltmacro
"#
);
