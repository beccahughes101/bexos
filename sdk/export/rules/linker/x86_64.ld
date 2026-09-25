EXTERN(__tls_get_addr)
ENTRY(_start)
PHDRS { text PT_LOAD FLAGS(5); data PT_LOAD FLAGS(6); bss PT_LOAD FLAGS(6); tls PT_TLS FLAGS(4); }
SECTIONS {
    /* Keep executable addresses in the signed 32-bit range used by the
       x86_64 Linux Rust standard library small code model. */
    . = 0x40000000;
    .text ALIGN(4K) : { *(.text._start) *(.text .text.*) } :text
    .rodata ALIGN(4K) : { *(.rodata .rodata.*) } :text
    .tdata ALIGN(4K) : {
        __bexos_tls_start = .;
        *(.tdata .tdata.*)
        __bexos_tls_file_end = .;
    } :data :tls
    .tbss (NOLOAD) : ALIGN(16) {
        *(.tbss .tbss.*)
        . = ALIGN(16);
        __bexos_pthread_local = .;
        . += 528;
        __bexos_tls_end = .;
    } :data :tls
    __bexos_tls_alignment = MAX(ALIGNOF(.tdata), ALIGNOF(.tbss));
    .data ALIGN(4K) : { *(.data .data.*) *(.got .got.*) } :data
    .bss ALIGN(4K) (NOLOAD) : { *(.bss .bss.*) *(COMMON) } :bss
    /DISCARD/ : { *(.comment) *(.eh_frame .eh_frame.*) }
}
