/* Mesa Venus renderer transport for capability-granted BexOS channels/VMOs. */
#include "internal.h"
#include "virtio-gpu/venus_hw.h"

/* Fixture-only setup, called before any Vulkan instance or renderer worker. */
void bexos_venus_enable_diagnostics(void)
{
    vn_env_init();
    vn_env.debug |= VN_DEBUG_INIT | VN_DEBUG_RESULT;
}

void bexos_renderer_mark_lost(struct bexos_renderer *renderer)
{
    pthread_mutex_lock(&renderer->mutex);
    atomic_store_explicit(&renderer->lost, true, memory_order_release);
    pthread_cond_broadcast(&renderer->changed);
    pthread_mutex_unlock(&renderer->mutex);
}

static void destroy(struct vn_renderer *base, const VkAllocationCallbacks *alloc)
{
    struct bexos_renderer *renderer = (void *)base;
    if (bexos_venus_close(renderer->context) != VK_SUCCESS)
        vn_log(NULL, "BexOS Venus context retained after failed release");
    pthread_cond_destroy(&renderer->changed);
    pthread_mutex_destroy(&renderer->mutex);
    vk_free(alloc, renderer);
}

static VkResult submit(struct vn_renderer *base, const struct vn_renderer_submit *submit)
{
    struct bexos_renderer *renderer = (void *)base;
    if (atomic_load_explicit(&renderer->lost, memory_order_acquire))
        return VK_ERROR_DEVICE_LOST;
    for (uint32_t i = 0; i < submit->batch_count; ++i) {
        const struct vn_renderer_submit_batch *batch = &submit->batches[i];
        uint64_t completion;
        VkResult result = bexos_venus_submit(renderer->context, batch->ring_idx,
                                             batch->cs_data, batch->cs_size, &completion);
        if (result != VK_SUCCESS) {
            bexos_renderer_mark_lost(renderer);
            return result;
        }
        /* The transport only returns success after this ring's completion
         * fence. Never substitute command receipt or a display timer here. */
        pthread_mutex_lock(&renderer->mutex);
        for (uint32_t j = 0; j < batch->sync_count; ++j) {
            struct bexos_sync *sync = (void *)batch->syncs[j];
            atomic_store_explicit(&sync->value, batch->sync_values[j], memory_order_release);
        }
        pthread_cond_broadcast(&renderer->changed);
        pthread_mutex_unlock(&renderer->mutex);
    }
    return VK_SUCCESS;
}

static VkResult wait_syncs(struct vn_renderer *base, const struct vn_renderer_wait *wait)
{
    struct bexos_renderer *renderer = (void *)base;
    const uint64_t start = os_time_get_nano();
    const uint64_t deadline = wait->timeout > UINT64_MAX - start ? UINT64_MAX : start + wait->timeout;
    const struct timespec time = { .tv_sec = deadline / 1000000000, .tv_nsec = deadline % 1000000000 };
    VkResult result;
    pthread_mutex_lock(&renderer->mutex);
    do {
        if (atomic_load_explicit(&renderer->lost, memory_order_acquire)) {
            result = VK_ERROR_DEVICE_LOST;
            break;
        }
        uint32_t ready = 0;
        for (uint32_t i = 0; i < wait->sync_count; ++i) {
            struct bexos_sync *sync = (void *)wait->syncs[i];
            ready += atomic_load_explicit(&sync->value, memory_order_acquire) >= wait->sync_values[i];
        }
        if (ready == wait->sync_count || (wait->wait_any && ready)) {
            result = VK_SUCCESS;
            break;
        }
        if (!wait->timeout || (wait->timeout != UINT64_MAX &&
            (uint64_t)os_time_get_nano() - start >= wait->timeout)) {
            result = VK_TIMEOUT;
            break;
        }
        int status = wait->timeout == UINT64_MAX
            ? pthread_cond_wait(&renderer->changed, &renderer->mutex)
            : pthread_cond_clockwait(&renderer->changed, &renderer->mutex, CLOCK_MONOTONIC, &time);
        if (status && status != ETIMEDOUT) {
            result = VK_ERROR_DEVICE_LOST;
            break;
        }
    } while (true);
    pthread_mutex_unlock(&renderer->mutex);
    return result;
}

VkResult vn_renderer_create_bexos(struct vn_instance *instance,
                                  const VkAllocationCallbacks *alloc,
                                  struct vn_renderer **out)
{
    struct bexos_renderer *renderer = vk_zalloc(alloc, sizeof(*renderer), VN_DEFAULT_ALIGN,
                                               VK_SYSTEM_ALLOCATION_SCOPE_INSTANCE);
    if (!renderer)
        return VK_ERROR_OUT_OF_HOST_MEMORY;
    if (pthread_mutex_init(&renderer->mutex, NULL)) {
        vk_free(alloc, renderer);
        return VK_ERROR_INITIALIZATION_FAILED;
    }
    if (pthread_cond_init(&renderer->changed, NULL)) {
        pthread_mutex_destroy(&renderer->mutex);
        vk_free(alloc, renderer);
        return VK_ERROR_INITIALIZATION_FAILED;
    }
    struct virgl_renderer_capset_venus capset = {0};
    VkResult result = bexos_venus_open(&renderer->context, &capset, sizeof(capset));
    if (result != VK_SUCCESS) {
        pthread_cond_destroy(&renderer->changed);
        pthread_mutex_destroy(&renderer->mutex);
        vk_free(alloc, renderer);
        return result;
    }
    if (!capset.wire_format_version || !capset.supports_blob_id_0 ||
        !capset.supports_multiple_timelines || !capset.allow_vk_wait_syncs) {
        destroy(&renderer->base, alloc);
        return VK_ERROR_INITIALIZATION_FAILED;
    }
    atomic_init(&renderer->lost, false);
    renderer->base.info = (struct vn_renderer_info) {
        .pci = {.vendor_id = 0x1af4, .device_id = 0x1050},
        .max_timeline_count = 64,
        .wire_format_version = capset.wire_format_version,
        .vk_xml_version = capset.vk_xml_version,
        .vk_ext_command_serialization_spec_version = capset.vk_ext_command_serialization_spec_version,
        .vk_mesa_venus_protocol_spec_version = capset.vk_mesa_venus_protocol_spec_version,
    };
    memcpy(renderer->base.info.vk_extension_mask, capset.vk_extension_mask1,
           sizeof(capset.vk_extension_mask1));
    renderer->base.ops = (struct vn_renderer_ops) {.destroy = destroy, .submit = submit, .wait = wait_syncs};
    renderer->base.shmem_ops = bexos_shmem_ops;
    renderer->base.bo_ops = bexos_bo_ops;
    renderer->base.sync_ops = bexos_sync_ops;
    *out = &renderer->base;
    return VK_SUCCESS;
}
