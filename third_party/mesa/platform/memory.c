/* Coherent blob mapping and resource ownership for Mesa Venus on BexOS. */
#include "internal.h"

static struct vn_renderer_shmem *create_shmem(struct vn_renderer *base, size_t size)
{
    struct bexos_renderer *renderer = (void *)base;
    struct bexos_shmem *memory = calloc(1, sizeof(*memory));
    if (!memory)
        return NULL;
    if (bexos_venus_memory_create(renderer->context, size, 0, 1, &memory->memory) != VK_SUCCESS) {
        free(memory);
        return NULL;
    }
    memory->base = (struct vn_renderer_shmem) {
        .refcount = VN_REFCOUNT_INIT(1),
        .res_id = memory->memory.resource,
        .mmap_size = memory->memory.size,
        .mmap_ptr = (void *)memory->memory.address,
    };
    return &memory->base;
}
static void destroy_shmem(struct vn_renderer *base, struct vn_renderer_shmem *shmem)
{
    struct bexos_renderer *renderer = (void *)base;
    struct bexos_shmem *memory = (void *)shmem;
    if (bexos_venus_memory_release(renderer->context, &memory->memory) != VK_SUCCESS)
        bexos_renderer_mark_lost(renderer);
    free(memory);
}
static VkResult create_bo(struct vn_renderer *base, VkDeviceSize size, vn_object_id id,
                          VkMemoryPropertyFlags flags, VkExternalMemoryHandleTypeFlags external,
                          struct vn_renderer_bo **out)
{
    if (external)
        return VK_ERROR_INVALID_EXTERNAL_HANDLE;
    struct bexos_renderer *renderer = (void *)base;
    struct bexos_bo *bo = calloc(1, sizeof(*bo));
    if (!bo)
        return VK_ERROR_OUT_OF_HOST_MEMORY;
    VkResult result = bexos_venus_memory_create(renderer->context, size, id,
        (flags & VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT) != 0, &bo->memory);
    if (result != VK_SUCCESS) {
        free(bo);
        return result;
    }
    bo->base = (struct vn_renderer_bo) {
        .refcount = VN_REFCOUNT_INIT(1), .res_id = bo->memory.resource,
        .mmap_size = bo->memory.size, .mmap_ptr = (void *)bo->memory.address,
    };
    *out = &bo->base;
    return VK_SUCCESS;
}
static VkResult import_bo(struct vn_renderer *renderer, VkDeviceSize size, int fd,
                          VkMemoryPropertyFlags flags, struct vn_renderer_bo **out)
{
    return VK_ERROR_INVALID_EXTERNAL_HANDLE;
}
static bool destroy_bo(struct vn_renderer *base, struct vn_renderer_bo *memory)
{
    struct bexos_renderer *renderer = (void *)base;
    struct bexos_bo *bo = (void *)memory;
    if (bexos_venus_memory_release(renderer->context, &bo->memory) != VK_SUCCESS)
        bexos_renderer_mark_lost(renderer);
    free(bo);
    return true;
}
static int export_bo(struct vn_renderer *renderer, struct vn_renderer_bo *bo) { return -1; }
static void *map_bo(struct vn_renderer *renderer, struct vn_renderer_bo *bo) { return bo->mmap_ptr; }
static void flush_bo(struct vn_renderer *renderer, struct vn_renderer_bo *bo, VkDeviceSize offset, VkDeviceSize size)
{
    /* The BexOS transport admits only coherent host mappings. */
    atomic_thread_fence(memory_order_release);
}
static void invalidate_bo(struct vn_renderer *renderer, struct vn_renderer_bo *bo, VkDeviceSize offset, VkDeviceSize size)
{
    atomic_thread_fence(memory_order_acquire);
}
    const struct vn_renderer_shmem_ops bexos_shmem_ops = {.create = create_shmem, .destroy = destroy_shmem};
    const struct vn_renderer_bo_ops bexos_bo_ops = {
        .create_from_device_memory = create_bo, .create_from_dma_buf = import_bo,
        .destroy = destroy_bo, .export_dma_buf = export_bo, .map = map_bo,
        .flush = flush_bo, .invalidate = invalidate_bo,
    };
