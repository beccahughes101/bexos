/* Version 1 of lib/secure_monitor_abi's register-only x86 transport. */
#pragma once
#include <stdint.h>
#include <stddef.h>
#define BEXOS_MONITOR_HEADER UINT64_C(0x4245584d00010002)
#define BEXOS_MONITOR_FETCH UINT64_C(0x102)
#define BEXOS_MONITOR_COMPLETE UINT64_C(0x103)
#define BEXOS_MONITOR_IS_REGISTERED UINT64_C(0x104)
#define BEXOS_MONITOR_ROOT_FETCH UINT64_C(0x105)
#define BEXOS_MONITOR_ROOT_COMPLETE UINT64_C(0x106)
#define BEXOS_MONITOR_BUFFER_SIZE 65536
#define BEXOS_BOOT_RPMB_RELEASE UINT32_C(0x80000000)
#define BEXOS_ROOT_JOURNAL UINT32_C(0x80000001)
#define BEXOS_ROOT_BOOT_SELECTION UINT32_C(0x80000002)
#define BEXOS_ROOT_GENERATION UINT32_C(0x80000003)
#define BEXOS_ROOT_MIGRATION_ABI UINT32_C(0x80000004)
#define BEXOS_ROOT_SERVICE_PROBE UINT32_C(0x80000005)
long monitor_journal(unsigned char* bytes, size_t length, size_t capacity);
long monitor_probe_services(void);
int bexos_boot_rpmb_release(void);
struct monitor_reply { uint64_t status, ticket, handle, operation, length, capacity; };
long monitor_root_request(struct monitor_reply request, unsigned char* bytes);
void monitor_root_worker_start(void);
static inline struct monitor_reply monitor_call(uint64_t operation, uint64_t arg0, uint64_t arg1) {
    uint64_t a = BEXOS_MONITOR_HEADER, b = operation, c = arg0, d = arg1, s = 0, di = 0;
    register uint64_t r8 __asm__("r8") = 0;
    register uint64_t r9 __asm__("r9") = 0;
    __asm__ volatile("vmmcall" : "+a"(a), "+b"(b), "+c"(c), "+d"(d), "+S"(s), "+D"(di), "+r"(r8), "+r"(r9) :: "memory", "cc");
    return (struct monitor_reply){a, b, c, d, s, di};
}
long monitor_ql_request(uint64_t handle, uint32_t operation, size_t length, size_t capacity, unsigned char *bytes);
void monitor_ql_collect(void);
