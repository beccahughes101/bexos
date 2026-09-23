ENTRY(_start)
SECTIONS {
    . = 0x0e200000;
    .text : { *(.text.entry) }
    . = ALIGN(4096);
    owner_start_address = .;
    .owner : {
        *(.text.owner)
        *(.text .text.*)
        *(.rodata .rodata.*)
        *(.data .data.*)
    }
    owner_copy_end = .;
    .bss (NOLOAD) : { *(.bss .bss.*) *(COMMON) }
    owner_end = .;
    . = ALIGN(4096);
    .trusty : { *(.trusty) }
    ASSERT(. - 0x0e200000 <= 0x00e00000, "signed owner and Trusty exceed TF-A secure load arena")
    /DISCARD/ : { *(.comment) *(.note*) *(.eh_frame*) }
}
