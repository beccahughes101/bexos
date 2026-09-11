/* Embedded monitor bytes and segment descriptors are covered by this EFI
 * image's Authenticode signature. Never load an external unchecked payload. */
#include "efi.h"
#include "monitor_payload.h"
extern const u8 monitor_payload[];
static u8 memory_map[64 * 1024] __attribute__((aligned(16)));

static void hex(u64 value) {
    char text[18];
    for (unsigned i = 0; i < 16; ++i)
        text[i] = "0123456789abcdef"[(value >> ((15 - i) * 4)) & 15];
    text[16] = '\n'; text[17] = 0;
    log(text);
}

static void reservation_failure(BootServices *boot, u64 address, u64 pages, u64 status) {
    log("efi-loader: reservation address/pages/status\n");
    hex(address); hex(pages); hex(status);
    u64 size = sizeof(memory_map), key, stride;
    unsigned version;
    if (boot->get_memory_map(&size, memory_map, &key, &stride, &version) || stride < 40)
        return;
    for (u64 offset = 0; offset + stride <= size; offset += stride) {
        const u64 *descriptor = (const u64 *)(memory_map + offset);
        u64 start = descriptor[1], end = start + descriptor[3] * 4096;
        if (start < address + pages * 4096 && address < end) {
            log("efi-loader: overlapping EFI type/base/pages\n");
            hex((unsigned)descriptor[0]); hex(start); hex(descriptor[3]);
        }
    }
}

u64 load_monitor(void *image, SystemTable *system) {
    u64 base = MONITOR_BASE;
    u64 status = system->boot->allocate_pages(2, 1, MONITOR_PAGES, &base);
    if (status || base != MONITOR_BASE) {
        log("efi-loader: FAILED cannot reserve monitor memory\n");
        return 0x8000000000000009ULL;
    }
    for (unsigned i = 0; i < MONITOR_BANKS; ++i) {
        u64 bank = domain_banks[i].address;
        status = system->boot->allocate_pages(2, 2, domain_banks[i].pages, &bank);
        if (status || bank != domain_banks[i].address) {
            reservation_failure(system->boot, domain_banks[i].address, domain_banks[i].pages, status);
            log("efi-loader: FAILED cannot reserve isolated domain RAM\n");
            return 0x8000000000000009ULL;
        }
    }
    for (unsigned i = 0; i < MONITOR_SEGMENTS; ++i) {
        const struct monitor_segment *segment = &monitor_segments[i];
        u8 *destination = (u8 *)(u64)segment->address;
        for (unsigned j = 0; j < segment->filesz; ++j)
            destination[j] = monitor_payload[segment->offset + j];
        for (unsigned j = segment->filesz; j < segment->memsz; ++j)
            destination[j] = 0;
    }
    for (unsigned attempt = 0; attempt < 4; ++attempt) {
        u64 size = sizeof(memory_map), key = 0, descriptor_size = 0;
        unsigned descriptor_version = 0;
        status = system->boot->get_memory_map(&size, memory_map, &key,
                                              &descriptor_size, &descriptor_version);
        if (status || descriptor_size < 40 || descriptor_version != 1) break;
        status = system->boot->exit_boot_services(image, key);
        if (status == 0) {
            __asm__ volatile("cli" ::: "memory");
            log("efi-loader: authenticated monitor reserved; boot services exited\n");
            /* Version 1 x86 entry; probe.c already verified Secure Boot and
             * SMM-backed variable protection before allowing load_monitor. */
            ((void (__attribute__((sysv_abi)) *)(u64, u64, u64))(u64)MONITOR_ENTRY)(
                0x4245584500010002ULL, 3, 0);
            for (;;) __asm__ volatile("cli; hlt");
        }
        if (status != 0x8000000000000002ULL) break;
    }
    /* Firmware may be partially shut down after an unsuccessful exit. Do not
     * return into its image dispatcher or call any further boot services. */
    log("efi-loader: FAILED boot-services exit rejected\n");
    for (;;) __asm__ volatile("cli; hlt");
}
