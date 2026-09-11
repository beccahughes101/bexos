#ifndef BEXOS_VENUS_INTERNAL_H
#define BEXOS_VENUS_INTERNAL_H
#include "vn_renderer.h"
#include "transport.h"
#include <pthread.h>
#include <errno.h>
struct bexos_renderer {
    struct vn_renderer base;
    uint32_t context;
    atomic_bool lost;
    pthread_mutex_t mutex;
    pthread_cond_t changed;
};
struct bexos_shmem {
    struct vn_renderer_shmem base;
    struct bexos_venus_memory memory;
};
struct bexos_bo {
    struct vn_renderer_bo base;
    struct bexos_venus_memory memory;
};
struct bexos_sync {
    struct vn_renderer_sync base;
    atomic_uint_fast64_t value;
};

extern const struct vn_renderer_shmem_ops bexos_shmem_ops;
extern const struct vn_renderer_bo_ops bexos_bo_ops;
extern const struct vn_renderer_sync_ops bexos_sync_ops;
void bexos_renderer_mark_lost(struct bexos_renderer *renderer);
#endif
