/* Only the root monitor can submit this operation. Kernel-origin IPC retains
 * upstream UUID authorization; normal QL clients keep their zero UUID. */
#include "transport.h"
#include <err.h>
#include <lib/ktipc/ktipc.h>
#include <lib/trusty/ipc.h>
#include <lib/trusty/uuid.h>
#include <platform.h>

/* Cold TP startup discovers RPMB geometry and authenticates its superblock.
 * Bound the complete exchange by the root's preparation budget, including
 * both connection and response waits, rather than imposing a one-second boot
 * storage deadline. A missing response to a commit remains indeterminate. */
static int wait_before(struct handle* channel, uint32_t* events, lk_time_t started) {
    lk_time_t elapsed = current_time() - started;
    if (elapsed >= 30000) return ERR_TIMED_OUT;
    return handle_wait(channel, events, 30000 - elapsed);
}

long monitor_journal(unsigned char* bytes, size_t length, size_t capacity) {
    const char* port;
    if (length == 96 && capacity == 96) port = "com.bexos.orchestrator.replacement";
    else if (length == 256 && capacity == 512) port = "com.bexos.orchestrator.boot-selection";
    else return ERR_INVALID_ARGS;
    lk_time_t started = current_time();
    struct handle* channel = NULL;
    int result = ipc_port_connect_async(&kernel_uuid, port,
        IPC_PORT_PATH_MAX, IPC_CONNECT_WAIT_FOR_PORT, &channel);
    if (result < 0) return result;
    uint32_t events = 0;
    result = wait_before(channel, &events, started);
    if (result < 0) goto done;
    if (!(events & IPC_HANDLE_POLL_READY) || (events & IPC_HANDLE_POLL_HUP)) {
        result = ERR_ACCESS_DENIED;
        goto done;
    }
    result = ktipc_send(channel, bytes, length);
    if (result < 0) goto done;
    if ((size_t)result != length) { result = ERR_BAD_LEN; goto done; }
    result = wait_before(channel, &events, started);
    if (result < 0) goto done;
    if (!(events & IPC_HANDLE_POLL_MSG)) { result = ERR_CHANNEL_CLOSED; goto done; }
    result = ktipc_recv(channel, capacity, bytes, capacity);
done:
    handle_decref(channel);
    return result;
}
