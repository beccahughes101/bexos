/* BexOS capability transport ABI. No Rust allocation or renderer pointer crosses
 * this boundary. Memory leases own their VMO mapping until release succeeds. */
#ifndef BEXOS_VENUS_TRANSPORT_H
#define BEXOS_VENUS_TRANSPORT_H
#include <stddef.h>
#include <stdint.h>
#include <vulkan/vulkan.h>

struct bexos_venus_memory {
    uint32_t resource;
    uint32_t host_visible;
    uint64_t size;
    uintptr_t address;
    uint64_t lease;
};

VkResult bexos_venus_open(uint32_t *context, void *capset, size_t capset_size);
VkResult bexos_venus_close(uint32_t context);
VkResult bexos_venus_submit(uint32_t context, uint32_t ring, const void *bytes,
                           size_t size, uint64_t *completion);
VkResult bexos_venus_memory_create(uint32_t context, uint64_t size, uint64_t blob_id,
                                  uint32_t map, struct bexos_venus_memory *memory);
VkResult bexos_venus_memory_release(uint32_t context, struct bexos_venus_memory *memory);
#endif
