ENTRY(_start)

PHDRS
{
    note PT_NOTE FLAGS(4);
    text PT_LOAD FLAGS(5);
    rodata PT_LOAD FLAGS(4);
    data PT_LOAD FLAGS(6);
}

SECTIONS
{
    .note.gnu.property 0 : ALIGN(8) {
        LONG(4)
        LONG(16)
        LONG(5)
        BYTE(0x47) BYTE(0x4e) BYTE(0x55) BYTE(0)
        . = ALIGN(8);
        LONG(0xc0000000)
        LONG(4)
        LONG(3)
        LONG(0)
        . = ALIGN(8);
        KEEP(*(.note.gnu.property .note.gnu.property.*))
    } :note

    . = 0x40200000;

    __kernel_start = .;

    . = ALIGN(4K);
    __kernel_text_start = .;
    .text : ALIGN(4K) {
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
    __free_memory_end = 0x48000000;

    /DISCARD/ : {
        *(.comment)
        *(.eh_frame)
    }
}
