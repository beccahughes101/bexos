OUTPUT_FORMAT(elf32-i386)
ENTRY(_start)
SECTIONS {
  /* Keep the loader outside both the Trusty and BexOS payload arenas. */
  . = 0x30000000;
  .text : { KEEP(*(.multiboot)) *(.text.boot) *(.text*) }
  .rodata : { *(.rodata*) }
  .data : { *(.data*) }
  .bss (NOLOAD) : { *(.bss*) *(COMMON) }
  /* The monitor consumes the embedded payload before it initializes the
   * normal-world bank. Q35 low RAM ends at the PCI hole at 0x80000000; retain
   * the final 16 MiB as a hard guard against firmware allocations. */
  ASSERT(. <= 0x7f000000, "Multiboot loader exceeds reserved Q35 low RAM")
  /DISCARD/ : { *(.comment) *(.eh_frame*) *(.note*) }
}
