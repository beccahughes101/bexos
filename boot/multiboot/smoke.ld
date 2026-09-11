ENTRY(_start)
SECTIONS {
  . = 0x200000;
  .text : { KEEP(*(.multiboot2)) *(.text*) }
  /DISCARD/ : { *(.note*) *(.comment*) }
}
