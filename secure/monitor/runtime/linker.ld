/* Version-one resident hardware layout. Replacement code may reclaim the
 * image below 0x18000000 only after transferring its software state. These
 * hardware-referenced objects and interrupt entry points remain resident.
 * The EFI loader reserves every PT_LOAD extent, including zero-filled tails.
 */
ENTRY(_start)
SECTIONS {
  . = 0x04000000;
  .boot : { KEEP(*(.text.boot)) }
  ASSERT(. <= 0x04001000, "monitor cold entry overflow")
  .resume 0x04001000 : { KEEP(*(.text.resume)) }
  ASSERT(. <= 0x04002000, "monitor resume entry overflow")
  .monitor_abi 0x04002000 : { KEEP(*(.monitor.abi)) }
  .text : ALIGN(4096) { *(.text .text.*) }
  .rodata : ALIGN(4096) { *(.rodata .rodata.*) }
  .data : ALIGN(4096) { *(.data .data.*) }
  .bss : ALIGN(4096) { *(.bss .bss.*) *(COMMON) }
  ASSERT(. <= 0x18000000, "monitor image overlaps resident hardware region")

  /DISCARD/ : { *(.eh_frame*) *(.comment*) }
}
