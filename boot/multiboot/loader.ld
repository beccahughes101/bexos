OUTPUT_FORMAT(elf32-i386)
ENTRY(_start)
SECTIONS {
  /* Keep the loader outside both the Trusty and BexOS payload arenas. */
  . = 0x30000000;
  .text : { KEEP(*(.multiboot)) *(.text.boot) *(.text*) }
  .rodata : { *(.rodata*) }
  .data : { *(.data*) }
  .bss (NOLOAD) : { *(.bss*) *(COMMON) }
  ASSERT(. <= 0x3f000000, "Multiboot loader exceeds reserved Q35 RAM")
  /DISCARD/ : { *(.comment) *(.eh_frame*) *(.note*) }
}
