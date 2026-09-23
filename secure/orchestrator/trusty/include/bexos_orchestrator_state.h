#ifndef BEXOS_ORCHESTRATOR_STATE_H
#define BEXOS_ORCHESTRATOR_STATE_H

#include <stddef.h>
#include <stdint.h>

#include "bexos_orchestrator_protocol.h"

struct bexos_orchestrator_state {
    uint32_t kernel_slot;
    uint64_t trusty_generation;
    uint64_t migration_abi;
    uint32_t fixture_mode;
};

void bexos_orchestrator_init(struct bexos_orchestrator_state* state);
size_t bexos_orchestrator_dispatch(struct bexos_orchestrator_state* state,
                                   const void* request,
                                   size_t request_size,
                                   void* response,
                                   size_t response_size);

#endif
