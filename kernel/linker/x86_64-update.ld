ENTRY(kernel_transplant_entry)

PHDRS
{
    api PT_LOAD FLAGS(4);
    text PT_LOAD FLAGS(5);
    rodata PT_LOAD FLAGS(4);
    data PT_LOAD FLAGS(6);
}

SECTIONS
{
    . = 0x10000000;

    __kernel_start = .;

    /* The ABI header precedes text; sharing rodata's program header would
       span the intervening executable segment and create overlapping loads. */
    .transplant_api : { KEEP(*(.transplant_api)) } :api

    . = ALIGN(4K);
    __kernel_text_start = .;
    .text : ALIGN(4K) {
        KEEP(*(.multiboot2))
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
        . = ALIGN(4K);
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
    __free_memory_end = 0x11000000;

    /DISCARD/ : {
        *(.comment)
        *(.eh_frame)
    }
}
