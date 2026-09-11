ENTRY(_start)

SECTIONS
{
    . = 0x60000000;
    .text : ALIGN(4096) { KEEP(*(.text._start)) *(.text .text.*) }
    .rodata : ALIGN(4096) { *(.rodata .rodata.*) }
    .data : ALIGN(4096) { *(.data .data.*) }
    .bss (NOLOAD) : ALIGN(4096) { __bss_start = .; *(.bss .bss.*) *(COMMON) __bss_end = .; }
    .stack (NOLOAD) : ALIGN(16) {
        __stack_bottom = .;
        . += 64K;
        __stack_top = .;
    }
    /DISCARD/ : { *(.comment) *(.note*) *(.eh_frame*) }
}
