ENTRY(_start)
SECTIONS {
    . = 0;
    .text : { *(.text.entry) *(.text*) }
    ASSERT(. <= 0x10000, "secure owner bootstrap image too large")
    /DISCARD/ : { *(.comment) *(.note*) *(.eh_frame*) }
}

