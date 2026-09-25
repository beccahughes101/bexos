ENTRY(_start)
PHDRS { text PT_LOAD FLAGS(5); data PT_LOAD FLAGS(6); bss PT_LOAD FLAGS(6); }
SECTIONS {
  . = 0x40000000;
  .text ALIGN(4K) : { *(.text._start) *(.text .text.*) } :text
  .rodata ALIGN(4K) : { *(.rodata .rodata.*) } :text
  .data ALIGN(4K) : { *(.data .data.*) *(.got .got.*) } :data
  .bss ALIGN(4K) (NOLOAD) : { *(.bss .bss.*) *(COMMON) } :bss
  /DISCARD/ : { *(.comment) *(.eh_frame .eh_frame.*) }
}
