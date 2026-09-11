#include "include/bexos_orchestrator_state.h"

#include <string.h>

void bexos_orchestrator_init(struct bexos_orchestrator_state* state) {
    state->kernel_slot = BEXOS_SLOT_A;
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
    default:
        out.status = BEXOS_ORCHESTRATOR_UNSUPPORTED;
    }
done:
    memcpy(response, &out, sizeof(out));
    return sizeof(out);
}
