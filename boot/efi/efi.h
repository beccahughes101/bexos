#ifndef BEXOS_EFI_H
#define BEXOS_EFI_H
typedef unsigned long long u64;
typedef unsigned short u16;
typedef unsigned char u8;
typedef struct { unsigned a; u16 b, c; u8 d[8]; } Guid;
typedef u64 (__attribute__((ms_abi)) *GetVariable)(u16 *, Guid *, unsigned *, u64 *, void *);
typedef u64 (__attribute__((ms_abi)) *SetVariable)(u16 *, Guid *, unsigned, u64, void *);
typedef struct {
    u8 header[24];
    void *get_time, *set_time, *get_wakeup, *set_wakeup, *set_virtual, *convert;
    GetVariable get_variable;
    void *get_next_variable_name;
    SetVariable set_variable;
} RuntimeServices;
typedef struct {
    u8 header[24];
    void *raise_tpl, *restore_tpl;
    u64 (__attribute__((ms_abi)) *allocate_pages)(unsigned, unsigned, u64, u64 *);
    void *free_pages;
    u64 (__attribute__((ms_abi)) *get_memory_map)(u64 *, void *, u64 *, u64 *, unsigned *);
    void *allocate_pool, *free_pool;
    void *create_event, *set_timer, *wait_for_event, *signal_event, *close_event, *check_event;
    void *install_protocol, *reinstall_protocol, *uninstall_protocol, *handle_protocol;
    void *reserved, *register_protocol_notify, *locate_handle, *locate_device_path, *install_configuration_table;
    void *load_image, *start_image, *exit, *unload_image;
    u64 (__attribute__((ms_abi)) *exit_boot_services)(void *, u64);
} BootServices;
typedef struct {
    u8 header[24];
    u16 *vendor;
    unsigned revision, padding;
    void *console_in_handle, *console_in, *console_out_handle, *console_out;
    void *stderr_handle, *stderr_interface;
    RuntimeServices *runtime;
    BootServices *boot;
} SystemTable;
_Static_assert(__builtin_offsetof(BootServices, allocate_pages) == 40, "UEFI AllocatePages offset");
_Static_assert(__builtin_offsetof(BootServices, get_memory_map) == 56, "UEFI GetMemoryMap offset");
_Static_assert(__builtin_offsetof(BootServices, exit_boot_services) == 232, "UEFI ExitBootServices offset");
_Static_assert(__builtin_offsetof(SystemTable, boot) == 96, "UEFI BootServices offset");
static void log(const char *text) {
    while (*text) {
        __asm__ volatile("outb %0, %1" :: "a"(*text++), "Nd"((u16)0x3f8));
    }
}
#endif
