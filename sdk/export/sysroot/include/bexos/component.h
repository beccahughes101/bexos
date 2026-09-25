#ifndef BEXOS_COMPONENT_H
#define BEXOS_COMPONENT_H

#include <stdint.h>

#define BEXOS_COMPONENT_ABI_VERSION 1u
#define BEXOS_MIGRATION_STATE_VERSION 1u

struct bexos_startup_resource {
    uint32_t kind;
    uint32_t reserved;
    uint64_t resource_id;
    uint64_t base;
    uint64_t length;
    uint64_t flags;
    uint64_t handle;
};

unsigned int bexos_component_abi_version(void);
unsigned int bexos_migration_state_version(void);

#endif
