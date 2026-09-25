EXTERN(__tls_get_addr)
ENTRY(_start)
PHDRS { note PT_NOTE FLAGS(4); text PT_LOAD FLAGS(5); data PT_LOAD FLAGS(6); bss PT_LOAD FLAGS(6); tls PT_TLS FLAGS(4); }
SECTIONS {
    .note.gnu.property 0 : ALIGN(8) {
        LONG(4)
        LONG(16)
        LONG(5)
        BYTE(0x47) BYTE(0x4e) BYTE(0x55) BYTE(0)
        . = ALIGN(8);
        LONG(0xc0000000)
        LONG(4)
        LONG(3)
        LONG(0)
        . = ALIGN(8);
        KEEP(*(.note.gnu.property .note.gnu.property.*))
    } :note
    . = 0x80000000;
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
    .data ALIGN(4K) : { *(.data .data.*) *(.got .got.*) } :data
    .bss ALIGN(4K) (NOLOAD) : { *(.bss .bss.*) *(COMMON) } :bss
    /DISCARD/ : { *(.comment) *(.eh_frame .eh_frame.*) }
}
