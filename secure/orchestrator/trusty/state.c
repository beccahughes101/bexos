#include "include/bexos_orchestrator_state.h"

#include <string.h>

#ifndef BEXOS_TRUSTY_GENERATION
#define BEXOS_TRUSTY_GENERATION 1u
#endif
#ifndef BEXOS_TRUSTY_MIGRATION_ABI
#define BEXOS_TRUSTY_MIGRATION_ABI 1u
#endif
#ifndef BEXOS_TRUSTY_FIXTURE_MODE
#define BEXOS_TRUSTY_FIXTURE_MODE 0u
#endif

/* Authenticated candidate metadata consumed by permanent execution owners
 * before they admit the image. Keep this in a loaded segment and independent
 * of mutable TA state so architecture, generation and migration compatibility
 * are properties of the exact signed bytes. */
__attribute__((used, section(".bexos.firmware")))
const struct {
    uint8_t magic[8];
    uint64_t generation;
    uint64_t migration_abi;
    uint64_t fixture_mode;
} bexos_firmware_manifest = {
    .magic = {'B', 'E', 'X', 'A', 'R', 'M', '0', '1'},
    .generation = BEXOS_TRUSTY_GENERATION,
    .migration_abi = BEXOS_TRUSTY_MIGRATION_ABI,
    .fixture_mode = BEXOS_TRUSTY_FIXTURE_MODE,
};

static void value64(struct bexos_orchestrator_response* out, uint64_t value) {
    out->active_slot = (uint32_t)value;
    out->reserved = (uint32_t)(value >> 32);
}

void bexos_orchestrator_init(struct bexos_orchestrator_state* state) {
    state->kernel_slot = BEXOS_SLOT_A;
    state->trusty_generation = BEXOS_TRUSTY_GENERATION;
    state->migration_abi = BEXOS_TRUSTY_MIGRATION_ABI;
    state->fixture_mode = BEXOS_TRUSTY_FIXTURE_MODE;
}

size_t bexos_orchestrator_dispatch(struct bexos_orchestrator_state* state,
                                   const void* request,
                                   size_t request_size,
                                   void* response,
                                   size_t response_size) {
    struct bexos_orchestrator_response out = {
        .version = BEXOS_ORCHESTRATOR_PROTOCOL_VERSION,
        .status = BEXOS_ORCHESTRATOR_BAD_MESSAGE,
        .active_slot = 0,
        .reserved = 0,
    };
    if (response_size < sizeof(out)) return 0;
    if (request_size != sizeof(struct bexos_orchestrator_request)) goto done;
    const struct bexos_orchestrator_request* in = request;
    if (in->version != BEXOS_ORCHESTRATOR_PROTOCOL_VERSION) {
        out.status = BEXOS_ORCHESTRATOR_BAD_VERSION;
        goto done;
    }
    if (in->reserved != 0) goto done;
    switch (in->command) {
    case BEXOS_ORCHESTRATOR_MARK_COMPONENT_FAILED:
        if (in->component == BEXOS_COMPONENT_KERNEL) {
            state->kernel_slot = BEXOS_SLOT_B;
        } else if (in->component != BEXOS_COMPONENT_D1_DRIVER &&
                   in->component != BEXOS_COMPONENT_D2_DRIVER) {
            out.status = BEXOS_ORCHESTRATOR_NOT_FOUND;
            goto done;
        }
        out.status = BEXOS_ORCHESTRATOR_OK;
        out.active_slot = state->kernel_slot;
        break;
    case BEXOS_ORCHESTRATOR_GET_KERNEL_SLOT:
    case BEXOS_ORCHESTRATOR_GET_ACTIVE_COMPONENT:
        if (in->command == BEXOS_ORCHESTRATOR_GET_ACTIVE_COMPONENT &&
            in->component != BEXOS_COMPONENT_KERNEL &&
            in->component != BEXOS_COMPONENT_D1_DRIVER &&
            in->component != BEXOS_COMPONENT_D2_DRIVER) {
            out.status = BEXOS_ORCHESTRATOR_NOT_FOUND;
            goto done;
        }
        out.status = BEXOS_ORCHESTRATOR_OK;
        out.active_slot = state->kernel_slot;
        break;
    case BEXOS_ORCHESTRATOR_GET_TRUSTY_GENERATION:
        if (in->component != 0) goto done;
        out.status = BEXOS_ORCHESTRATOR_OK;
        value64(&out, state->trusty_generation);
        break;
    case BEXOS_ORCHESTRATOR_GET_MIGRATION_ABI:
        if (in->component != 0) goto done;
        out.status = BEXOS_ORCHESTRATOR_OK;
        value64(&out, state->migration_abi);
        break;
    case BEXOS_ORCHESTRATOR_GET_FIXTURE_MODE:
        if (in->component != 0) goto done;
        out.status = BEXOS_ORCHESTRATOR_OK;
        value64(&out, state->fixture_mode);
        break;
    default:
        out.status = BEXOS_ORCHESTRATOR_UNSUPPORTED;
    }
done:
    memcpy(response, &out, sizeof(out));
    return sizeof(out);
}
