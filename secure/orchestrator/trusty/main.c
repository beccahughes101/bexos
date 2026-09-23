#include "include/bexos_orchestrator_state.h"

#include <lib/tipc/tipc_srv.h>
#include <string.h>
#include <uapi/err.h>

#ifndef BEXOS_TRUSTY_FIXTURE_MODE
#define BEXOS_TRUSTY_FIXTURE_MODE 0u
#endif

static struct bexos_orchestrator_state state;
int bexos_journal_add_service(struct tipc_hset* hset);

static int on_message(const struct tipc_port* port, handle_t chan, void* ctx) {
    (void)port;
    (void)ctx;
    uint8_t request[BEXOS_ORCHESTRATOR_MAX_MESSAGE_SIZE];
    struct bexos_orchestrator_response response;
    int received = tipc_recv1(chan, sizeof(struct bexos_orchestrator_request), request, sizeof(request));
    if (received < 0) return received;
    size_t response_size = bexos_orchestrator_dispatch(&state, request, (size_t)received,
                                                        &response, sizeof(response));
    if (!response_size) return ERR_BAD_LEN;
    int sent = tipc_send1(chan, &response, response_size);
    return sent < 0 ? sent : NO_ERROR;
}

int main(void) {
#if BEXOS_TRUSTY_FIXTURE_MODE == 2
    __builtin_trap();
#elif BEXOS_TRUSTY_FIXTURE_MODE == 3
    static volatile unsigned hang;
    for (;;) hang++;
#endif
    static const struct tipc_port_acl acl = { .flags = IPC_PORT_ALLOW_NS_CONNECT };
    static const struct tipc_port port = {
        .name = BEXOS_ORCHESTRATOR_PORT,
        .msg_max_size = BEXOS_ORCHESTRATOR_MAX_MESSAGE_SIZE,
        .msg_queue_len = 1,
        .acl = &acl,
    };
    static const struct tipc_srv_ops ops = { .on_message = on_message };
    struct tipc_hset* hset = tipc_hset_create();
    if (!hset) return ERR_NO_MEMORY;
    bexos_orchestrator_init(&state);
    int rc = tipc_add_service(hset, &port, 1, 1, &ops);
    if (!rc) rc = bexos_journal_add_service(hset);
    return rc ? rc : tipc_run_event_loop(hset);
}
