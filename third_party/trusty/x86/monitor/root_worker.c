/* Root journal calls may wait for TP storage. Keep them off the QL worker
 * which delivers normal-world storage replies needed to complete that wait. */
#include "transport.h"
#include <debug.h>
#include <err.h>
#include <kernel/thread.h>
#include <kernel/vm.h>

static unsigned char root_scratch[BEXOS_MONITOR_BUFFER_SIZE] __attribute__((aligned(4096)));

long monitor_root_request(struct monitor_reply request, unsigned char* bytes) {
    if (request.handle != UINT64_MAX) return ERR_ACCESS_DENIED;
    if (request.operation == BEXOS_BOOT_RPMB_RELEASE) {
        return request.length == 1 && request.capacity == 1 && bytes[0] == 1
            ? bexos_boot_rpmb_release() : ERR_INVALID_ARGS;
    }
    if (request.operation == BEXOS_ROOT_JOURNAL || request.operation == BEXOS_ROOT_BOOT_SELECTION) {
        bool valid = request.operation == BEXOS_ROOT_JOURNAL
            ? request.length == 96 && request.capacity == 96
            : request.length == 256 && request.capacity == 512;
        return valid ? monitor_journal(bytes, request.length, request.capacity) : ERR_INVALID_ARGS;
    }
    if (request.operation == BEXOS_ROOT_GENERATION) {
        if (request.length != 8 || request.capacity != 8) return ERR_INVALID_ARGS;
        uint64_t generation = BEXOS_TRUSTY_GENERATION;
        for (unsigned i = 0; i < 8; ++i) bytes[i] = generation >> (i * 8);
        return 8;
    }
    if (request.operation == BEXOS_ROOT_MIGRATION_ABI) {
        if (request.length != 8 || request.capacity != 8) return ERR_INVALID_ARGS;
        uint64_t migration_abi = BEXOS_TRUSTY_MIGRATION_ABI;
        for (unsigned i = 0; i < 8; ++i) bytes[i] = migration_abi >> (i * 8);
        return 8;
    }
    if (request.operation == BEXOS_ROOT_SERVICE_PROBE) {
        if (request.length != 8 || request.capacity != 8) return ERR_INVALID_ARGS;
        uint64_t services = (uint64_t)monitor_probe_services();
        for (unsigned i = 0; i < 8; ++i) bytes[i] = services >> (i * 8);
        return 8;
    }
    return ERR_NOT_SUPPORTED;
}

static int root_worker(void* unused) {
    (void)unused;
    uint64_t physical = vaddr_to_paddr(root_scratch);
    for (;;) {
        struct monitor_reply request = monitor_call(BEXOS_MONITOR_ROOT_FETCH, physical, 0);
        if ((int64_t)request.status == -6) { thread_sleep(1); continue; }
        if (request.status) panic("root control fetch rejected");
        long result = monitor_root_request(request, root_scratch);
        struct monitor_reply completed = monitor_call(BEXOS_MONITOR_ROOT_COMPLETE, request.ticket, (uint64_t)result);
        if (completed.status && (int64_t)completed.status != -5) panic("root control completion rejected");
    }
}
void monitor_root_worker_start(void) {
    thread_t* thread = thread_create("bexos-root-control", root_worker, NULL, DEFAULT_PRIORITY, 16384);
    if (!thread) panic("create root control thread");
    thread_resume(thread);
    dprintf(ALWAYS, "bexos-trusty-x86: independent root control worker ready\n");
}
