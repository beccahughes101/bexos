#if defined(__aarch64__)
.text
.global _start
.type _start, %function
_start:
    bl main
1:  wfe
    b 1b
#elif defined(__x86_64__)
.text
.global _start
.type _start, @function
_start:
    andq $-16, %rsp
    call main
1:  pause
    jmp 1b
#endif
