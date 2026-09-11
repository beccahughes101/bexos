/* Firmware authentication probe. This is not the BexOS product loader. */
#include "efi.h"
#ifdef MONITOR_PAYLOAD
u64 load_monitor(void *image, SystemTable *system);
#endif
u64 __attribute__((ms_abi)) efi_main(void *image, SystemTable *system) {
    (void)image;
    log("efi-probe: entered image\n");
    Guid global = {0x8be4df61, 0x93ca, 0x11d2, {0xaa,0x0d,0x00,0xe0,0x98,0x03,0x2b,0x8c}};
    u16 secure_name[] = {'S','e','c','u','r','e','B','o','o','t',0};
    u16 setup_name[] = {'S','e','t','u','p','M','o','d','e',0};
    u8 secure = 0, setup = 1;
    u64 size = 1;
    u64 status = system->runtime->get_variable(secure_name, &global, 0, &size, &secure);
    size = 1;
    status |= system->runtime->get_variable(setup_name, &global, 0, &size, &setup);
    if (status || secure != 1 || setup != 0) {
        log("efi-probe: secure boot disabled; refusing execution\n");
        return 0x800000000000001aULL;
    }
    u16 pk_name[] = {'P','K',0};
    /* Well-formed authentication descriptor, future timestamp, missing PKCS7
     * signature: deletion must fail authentication, not just attribute checks. */
    u8 unsigned_delete[40] = {
        0x33,0x08,1,1,0,0,0,0, 0,0,0,0,0,0,0,0,
        24,0,0,0, 0,2,0xf1,0x0e,
        0x9d,0xd2,0xaf,0x4a,0xdf,0x68,0xee,0x49,
        0x8a,0xa9,0x34,0x7d,0x37,0x56,0x65,0xa7,
    };
    status = system->runtime->set_variable(pk_name, &global, 0x27,
                                         sizeof(unsigned_delete), unsigned_delete);
    if (status != 0x800000000000001aULL) {
        log("efi-probe: FAILED unauthenticated PK deletion did not return security violation\n");
        return 0x800000000000001aULL;
    }
    log("efi-probe: unauthenticated PK deletion rejected\n");
    /* The pinned four-MiB OVMF layout starts its variable store at ffc00000.
     * Try an Intel flash byte-program operation from outside SMM. The test
     * also runs with QEMU's flash protection disabled as a negative control. */
    volatile u8 *variable_start = (volatile u8 *)0xffc00064ULL;
    if (*variable_start != 0xaa) {
        log("efi-probe: FAILED unexpected variable flash layout\n");
        return 0x800000000000001aULL;
    }
    *variable_start = 0x40;
    *variable_start = 0;
    *variable_start = 0xff;
    if (*variable_start != 0xaa) {
        log("efi-probe: unprotected variable flash; refusing execution\n");
        return 0x800000000000001aULL;
    }
    log("efi-probe: non-SMM variable flash write blocked\n");
    log("efi-probe: firmware authenticated image; SecureBoot=1 SetupMode=0\n");
#ifdef MONITOR_PAYLOAD
    return load_monitor(image, system);
#endif
    for (;;) __asm__ volatile("cli; hlt");
}
