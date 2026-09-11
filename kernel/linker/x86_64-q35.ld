ENTRY(_start)

PHDRS
{
    text PT_LOAD FLAGS(5);
    rodata PT_LOAD FLAGS(4);
    data PT_LOAD FLAGS(6);
}

SECTIONS
{
    . = 0x02000000;

    __kernel_start = .;

    . = ALIGN(4K);
    __kernel_text_start = .;
    .text : ALIGN(4K) {
        KEEP(*(.multiboot .multiboot2))
        KEEP(*(.text.boot))
        *(.text .text.*)
    } :text
    . = ALIGN(4K);
    __kernel_text_end = .;

    __kernel_rodata_start = .;
    .rodata : ALIGN(4K) {
        *(.rodata .rodata.*)
    } :rodata
    . = ALIGN(4K);
    __kernel_rodata_end = .;

    __kernel_data_start = .;
    .data : ALIGN(4K) {
        *(.data .data.*)
        *(.got .got.*)
    } :data
    . = ALIGN(4K);
    __kernel_data_end = .;

    __kernel_bss_start = .;
    .bss (NOLOAD) : ALIGN(4K) {
        *(.bss.stack)
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        __bss_end = .;
    } :data
    . = ALIGN(4K);
    __kernel_bss_end = .;

    . = ALIGN(4K);
    __kernel_end = .;
    __free_memory_start = .;
    __free_memory_end = 0x20000000;

    /DISCARD/ : {
        *(.comment)
        *(.eh_frame)
    }
}
