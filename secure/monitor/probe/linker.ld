ENTRY(_start)
SECTIONS {
  . = 0x04000000;
  .text : { KEEP(*(.text.boot)) *(.text .text.*) }
  .rodata : ALIGN(4096) { *(.rodata .rodata.*) }
  .data : ALIGN(4096) { *(.data .data.*) }
  .bss : ALIGN(4096) { *(.bss .bss.*) *(COMMON) }
  /DISCARD/ : { *(.eh_frame*) *(.comment*) }
}
