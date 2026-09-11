ENTRY(_start)
SECTIONS {
  .entry 0x04000000 : { KEEP(*(.text.entry)) }
  ASSERT(. <= 0x04001000, "monitor policy entry overflow")
  .abi 0x04001000 : { KEEP(*(.monitor.abi)) }
  .text : ALIGN(4096) { *(.text .text.*) }
  .rodata : ALIGN(4096) { *(.rodata .rodata.*) }
  .data : ALIGN(4096) { *(.data .data.*) }
  .bss : ALIGN(4096) { *(.bss .bss.*) *(COMMON) }
  ASSERT(. <= 0x041f0000, "monitor policy overlaps private stack")
  /DISCARD/ : { *(.eh_frame*) *(.comment*) }
}
