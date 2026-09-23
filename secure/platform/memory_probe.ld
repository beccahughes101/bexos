ENTRY(_start)
SECTIONS {
    . = 0;
    .text : { *(.text.entry) *(.text*) }
    /DISCARD/ : { *(.comment) *(.note*) *(.eh_frame*) }
}
