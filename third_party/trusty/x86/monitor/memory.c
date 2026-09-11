/* Private bounce objects retain upstream QL endpoint authorization: QL assigns
 * zero_uuid to these NS clients, never kernel_uuid or a trusted-app identity.
 * No normal-world address is mapped into Trusty's address space. */
#include "transport.h"
#include <arch/mmu.h>
#include <err.h>
#include <kernel/vm.h>
#include <lib/extmem/extmem.h>
#include <lib/sm/sm_err.h>
#include <stdlib.h>
#include <string.h>
#include "tipc_dev_ql.h"

#define CLIENT 1
#define SLOTS 2
#define CREATE 0x3200001e
#define SHUTDOWN 0x3200001f
#define COMMAND 0x32000020
static struct { uint64_t handle; size_t capacity; } slots[SLOTS];
static unsigned char buffers[SLOTS][BEXOS_MONITOR_BUFFER_SIZE] __attribute__((aligned(4096)));

static void destroy(struct vmm_obj *vmm) {
    struct ext_mem_obj *object = (struct ext_mem_obj *)((unsigned char *)vmm - offsetof(struct ext_mem_obj, vmm_obj));
    free(object);
}
static struct vmm_obj_ops operations = {
    .check_flags = ext_mem_obj_check_flags,
    .get_page = ext_mem_obj_get_page,
    .destroy = destroy,
};

status_t ext_mem_get_vmm_obj(ext_mem_client_id_t client, ext_mem_obj_id_t id, uint64_t tag,
                           size_t size, struct vmm_obj **output, struct obj_ref *reference) {
    if (client != CLIENT || tag || !size) return ERR_ACCESS_DENIED;
    for (size_t i = 0; i < SLOTS; ++i) {
        if (slots[i].handle != id || !id || size > slots[i].capacity) continue;
        struct ext_mem_obj *object = calloc(1, sizeof(*object) + sizeof(struct ext_mem_page_run));
        if (!object) return ERR_NO_MEMORY;
        ext_mem_obj_initialize(object, reference, id, 0, &operations,
                               ARCH_MMU_FLAG_CACHED | ARCH_MMU_FLAG_PERM_NO_EXECUTE, 1);
        object->page_runs[0].paddr = vaddr_to_paddr(buffers[i]);
        object->page_runs[0].size = slots[i].capacity;
        *output = &object->vmm_obj;
        return NO_ERROR;
    }
    return ERR_NOT_FOUND;
}

static void release(size_t i) {
    /* Call only after upstream QL has released the persistent mapping. */
    memset(buffers[i], 0, sizeof(buffers[i]));
    slots[i].handle = 0;
    slots[i].capacity = 0;
}

void monitor_ql_collect(void) {
    for (size_t i = 0; i < SLOTS; ++i) {
        if (slots[i].handle && monitor_call(BEXOS_MONITOR_IS_REGISTERED, slots[i].handle, 0).status != 0) {
            if (ql_tipc_shutdown_device(CLIENT, slots[i].handle) == 0) release(i);
        }
    }
}

long monitor_ql_request(uint64_t handle, uint32_t operation, size_t length, size_t capacity, unsigned char *bytes) {
    if (!handle || !(handle >> 63) || !capacity || capacity > BEXOS_MONITOR_BUFFER_SIZE
        || (capacity & 4095) || !length || length > capacity) return SM_ERR_INVALID_PARAMETERS;
    size_t index = SLOTS;
    for (size_t i = 0; i < SLOTS; ++i) if (slots[i].handle == handle) index = i;
    if (operation == CREATE) {
        if (index != SLOTS || length != capacity) return SM_ERR_INVALID_PARAMETERS;
        for (size_t i = 0; i < SLOTS; ++i) if (!slots[i].handle) { index = i; break; }
        if (index == SLOTS) return SM_ERR_BUSY;
        slots[index].handle = handle;
        slots[index].capacity = capacity;
        memset(buffers[index], 0, sizeof(buffers[index]));
        long result = ql_tipc_create_device(CLIENT, handle, capacity, ARCH_MMU_FLAG_PERM_NO_EXECUTE);
        if (result) release(index);
        return result;
    }
    if (index == SLOTS || slots[index].capacity != capacity) return SM_ERR_INVALID_PARAMETERS;
    if (operation == SHUTDOWN) {
        long result = ql_tipc_shutdown_device(CLIENT, handle);
        if (!result) release(index);
        return result;
    }
    if (operation != COMMAND) return SM_ERR_NOT_SUPPORTED;
    memcpy(buffers[index], bytes, capacity);
    long result = ql_tipc_handle_cmd(CLIENT, handle, length, false);
    memcpy(bytes, buffers[index], capacity);
    return result;
}
