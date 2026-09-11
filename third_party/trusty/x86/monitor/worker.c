/* The same saved image remains bootable standalone. Only the in-tree monitor
 * advertises this versioned service; unknown hypervisors never receive calls. */
#include "transport.h"
#include <debug.h>
#include <kernel/thread.h>
#include <kernel/vm.h>
#include <lk/init.h>
#include <err.h>
static unsigned char scratch[BEXOS_MONITOR_BUFFER_SIZE] __attribute__((aligned(4096)));
static int worker(void *unused) {
    (void)unused;
    uint64_t physical = vaddr_to_paddr(scratch);
    for (;;) {
        monitor_ql_collect();
        struct monitor_reply request = monitor_call(BEXOS_MONITOR_FETCH, physical, 0);
        if ((int64_t)request.status == -6) { thread_sleep(1); continue; }
        if (request.status) panic("monitor transport fetch rejected");
        long result;
        if (request.operation >= BEXOS_BOOT_RPMB_RELEASE) {
            result = monitor_root_request(request, scratch);
        } else {
            result = monitor_ql_request(request.handle, request.operation, request.length, request.capacity, scratch);
        }
        struct monitor_reply completed = monitor_call(BEXOS_MONITOR_COMPLETE, request.ticket, (uint64_t)result);
        /* Revoked normal-world owners cannot receive late replies. */
        if (completed.status && (int64_t)completed.status != -5) panic("monitor transport completion rejected");
    }
}
static void initialize(uint level) {
    (void)level;
    uint32_t a = 0x40000100, b, c = 0, d;
    __asm__ volatile("cpuid" : "+a"(a), "=b"(b), "+c"(c), "=d"(d));
    /* Bit 0: normal boot services; bit 1: independent root control lane.
     * Older signed monitors retain the legacy single-worker dispatch. */
    if (a != 0x4245584d || b != 1 || c != 2 || (d & ~3u) != 0) return;
    if (d & 2u) monitor_root_worker_start();
    thread_t *thread = thread_create("bexos-monitor-ipc", worker, NULL, DEFAULT_PRIORITY, 16384);
    if (!thread) panic("create monitor IPC thread");
    thread_resume(thread);
    dprintf(ALWAYS, "bexos-trusty-x86: monitor IPC ready version=1\n");
}
LK_INIT_HOOK(bexos_monitor_ipc, initialize, LK_INIT_LEVEL_APPS);
