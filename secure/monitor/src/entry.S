.section .text
.global bexos_svm_enter
.type bexos_svm_enter,@function
bexos_svm_enter:
    pushfq
    cli
    push rbx
    push rbp
    push r12
    push r13
    push r14
    push r15
    push rdi
    push rsi
    push rdx
    sub rsp, 520
    fxsave64 [rsp]
    mov rax, rdx
    vmsave rax
    mov rax, rdi
    vmload rax
    mov rax, rsi
    fxrstor64 [rax + 112]
    mov rbx, [rax]
    mov rcx, [rax + 8]
    mov rdx, [rax + 16]
    mov rsi, [rax + 24]
    mov rdi, [rax + 32]
    mov rbp, [rax + 40]
    mov r8, [rax + 48]
    mov r9, [rax + 56]
    mov r10, [rax + 64]
    mov r11, [rax + 72]
    mov r12, [rax + 80]
    mov r13, [rax + 88]
    mov r14, [rax + 96]
    mov r15, [rax + 104]
    mov rax, [rsp + 536]
    vmrun rax
    vmsave rax
    mov rax, [rsp + 528]
    mov [rax], rbx
    mov [rax + 8], rcx
    mov [rax + 16], rdx
    mov [rax + 24], rsi
    mov [rax + 32], rdi
    mov [rax + 40], rbp
    mov [rax + 48], r8
    mov [rax + 56], r9
    mov [rax + 64], r10
    mov [rax + 72], r11
    mov [rax + 80], r12
    mov [rax + 88], r13
    mov [rax + 96], r14
    mov [rax + 104], r15
    fxsave64 [rax + 112]
    mov rax, [rsp + 520]
    vmload rax
    fxrstor64 [rsp]
    add rsp, 544
    pop r15
    pop r14
    pop r13
    pop r12
    pop rbp
    pop rbx
    popfq
    ret
.size bexos_svm_enter, .-bexos_svm_enter
