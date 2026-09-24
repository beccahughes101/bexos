/* Permanent recovery and execution nucleus. Candidate policy images are
 * separate ELFs linked at 0x04000000; no nucleus reference enters their bank. */
ENTRY(_start)
SECTIONS {
  .resident.payload 0x04200000 (NOLOAD) : { KEEP(*(.resident.payload)) }
  ASSERT(. <= 0x12400000, "permanent boot snapshots overlap firmware workspace")
  .resident.firmware 0x12400000 (NOLOAD) : { KEEP(*(.resident.firmware)) }
  ASSERT(. <= 0x16800000, "bounded firmware workspace overflow")
  .boot 0x18200000 : { KEEP(*(.text.boot)) }
  .text : ALIGN(4096) { *(.text .text.*) }
  .rodata : ALIGN(4096) { *(.rodata .rodata.*) }
  .data : ALIGN(4096) { *(.data .data.*) *(.got .got.*) }
  .bss : ALIGN(4096) { *(.bss .bss.*) *(COMMON) }
  ASSERT(. <= 0x20000000, "permanent nucleus overlaps Trusty RAM")
  /DISCARD/ : { *(.eh_frame*) *(.comment*) }
}
