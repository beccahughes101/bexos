/* Completion-backed timeline counters; external Linux handles are unsupported. */
#include "internal.h"

static VkResult create_sync(struct vn_renderer *renderer, uint64_t value, uint32_t flags,
                            struct vn_renderer_sync **out)
{
    if (flags & VN_RENDERER_SYNC_SHAREABLE)
        return VK_ERROR_INVALID_EXTERNAL_HANDLE;
    struct bexos_sync *sync = calloc(1, sizeof(*sync));
    if (!sync)
        return VK_ERROR_OUT_OF_HOST_MEMORY;
    atomic_init(&sync->value, value);
    *out = &sync->base;
    return VK_SUCCESS;
}
static VkResult import_sync(struct vn_renderer *renderer, int fd, bool sync_file, struct vn_renderer_sync **out)
{ return VK_ERROR_INVALID_EXTERNAL_HANDLE; }
static int export_sync(struct vn_renderer *renderer, struct vn_renderer_sync *sync, bool sync_file) { return -1; }
static void destroy_sync(struct vn_renderer *renderer, struct vn_renderer_sync *sync) { free(sync); }
static VkResult write_sync(struct vn_renderer *renderer, struct vn_renderer_sync *base, uint64_t value)
{
    struct bexos_sync *sync = (void *)base;
    struct bexos_renderer *owner = (void *)renderer;
    pthread_mutex_lock(&owner->mutex);
    atomic_store_explicit(&sync->value, value, memory_order_release);
    pthread_cond_broadcast(&owner->changed);
    pthread_mutex_unlock(&owner->mutex);
    return VK_SUCCESS;
}
static VkResult read_sync(struct vn_renderer *renderer, struct vn_renderer_sync *base, uint64_t *value)
{
    struct bexos_sync *sync = (void *)base;
    *value = atomic_load_explicit(&sync->value, memory_order_acquire);
    return VK_SUCCESS;
}

    const struct vn_renderer_sync_ops bexos_sync_ops = {
        .create = create_sync, .create_from_syncobj = import_sync, .destroy = destroy_sync,
        .export_syncobj = export_sync, .reset = write_sync, .read = read_sync, .write = write_sync,
    };
