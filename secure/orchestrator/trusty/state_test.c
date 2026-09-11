#include "include/bexos_orchestrator_state.h"

#include <assert.h>

int main(void) {
    struct bexos_orchestrator_state state;
    struct bexos_orchestrator_request request = {
        .version = BEXOS_ORCHESTRATOR_PROTOCOL_VERSION,
        .command = BEXOS_ORCHESTRATOR_GET_KERNEL_SLOT,
    };
    struct bexos_orchestrator_response response;
    bexos_orchestrator_init(&state);
    assert(bexos_orchestrator_dispatch(&state, &request, sizeof(request), &response, sizeof(response)) == sizeof(response));
    assert(response.status == BEXOS_ORCHESTRATOR_OK && response.active_slot == BEXOS_SLOT_A);
    request.command = BEXOS_ORCHESTRATOR_MARK_COMPONENT_FAILED;
    request.component = BEXOS_COMPONENT_KERNEL;
    bexos_orchestrator_dispatch(&state, &request, sizeof(request), &response, sizeof(response));
    assert(response.active_slot == BEXOS_SLOT_B);
    request.version = 99;
    bexos_orchestrator_dispatch(&state, &request, sizeof(request), &response, sizeof(response));
    assert(response.status == BEXOS_ORCHESTRATOR_BAD_VERSION);
    return 0;
}
