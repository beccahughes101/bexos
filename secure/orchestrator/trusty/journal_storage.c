/* One Trusty TP transaction commits image identity, slot and generation floor
 * together. Normal-world RPMB transport sees only authenticated opaque frames. */
#include "include/bexos_journal.h"
#include <lib/storage/storage.h>
#include <lib/tipc/tipc_srv.h>
#include <string.h>
#include <uapi/err.h>
#include <uapi/trusty_uuid.h>

#if defined(__aarch64__)
#define JOURNAL_ARCH 1u
#elif defined(__x86_64__)
#define JOURNAL_ARCH 2u
#else
#error Unsupported replacement architecture
#endif

static uint32_t transact(const uint8_t* request, uint8_t* response) {
    const uint32_t component = bexos_journal_word(request, 16);
    const uint32_t command = bexos_journal_word(request, 8);
    const char* name = component == 1 ? "replacement.trusty.v1" : "replacement.monitor.v1";
    storage_session_t session;
    file_handle_t file = 0;
    bool opened = false;
    uint32_t status = BEXOS_JOURNAL_UNAVAILABLE;
    if (storage_open_session(&session, STORAGE_CLIENT_TP_PORT) < 0) return status;
    int rc = storage_open_file(session, &file, name, 0, 0);
    if (rc == ERR_NOT_FOUND) {
        bexos_journal_initial(response, JOURNAL_ARCH, component);
    } else if (rc < 0) {
        goto done;
    } else {
        opened = true;
        storage_off_t length = 0;
        if (storage_get_file_size(file, &length) < 0 || length != BEXOS_JOURNAL_BYTES ||
            storage_read(file, 0, response, BEXOS_JOURNAL_BYTES) != BEXOS_JOURNAL_BYTES ||
            !bexos_journal_record_valid(response, JOURNAL_ARCH, component)) goto done;
    }
    if (command == BEXOS_JOURNAL_QUERY) {
        status = BEXOS_JOURNAL_OK;
        goto done;
    }
    /* Once boot selection exists, only its atomic two-component transaction
     * may update the compatibility journals. Do not permit a v1 caller to
     * bypass a pending trial or desynchronize the two protected formats. */
    file_handle_t selection;
    rc = storage_open_file(session, &selection, "replacement.boot.v2", 0, 0);
    if (rc >= 0) {
        storage_close_file(selection);
        status = BEXOS_JOURNAL_INVALID;
        goto done;
    }
    if (rc != ERR_NOT_FOUND) goto done;
    uint8_t next[BEXOS_JOURNAL_BYTES];
    status = bexos_journal_next(response, request, next);
    if (status != BEXOS_JOURNAL_OK || !memcmp(response, next, sizeof(next))) goto done;
    status = BEXOS_JOURNAL_UNAVAILABLE;
    if (!opened) {
        if (storage_open_file(session, &file, name,
            STORAGE_FILE_OPEN_CREATE | STORAGE_FILE_OPEN_CREATE_EXCLUSIVE, 0) < 0) goto done;
        opened = true;
    }
    /* Once a write is issued, never infer rollback from a lost response. The
     * resident owner must query authenticated state before retiring an owner. */
    status = BEXOS_JOURNAL_UNCERTAIN;
    if (storage_write(file, 0, next, sizeof(next), 0) != sizeof(next) ||
        storage_end_transaction(session, true) < 0) goto done;
    memcpy(response, next, sizeof(next));
    status = BEXOS_JOURNAL_OK;
done:
    if (opened) storage_close_file(file);
    /* Closing aborts any transaction not acknowledged as committed. This
     * never acknowledges filesystem repair or accepts rollback of TP state. */
    storage_close_session(session);
    return status;
}

static int on_message(const struct tipc_port* port, handle_t channel, void* context) {
    (void)port;
    (void)context;
    uint8_t request[BEXOS_JOURNAL_BYTES], response[BEXOS_JOURNAL_BYTES] = {0};
    int received = tipc_recv1(channel, sizeof(request), request, sizeof(request));
    if (received < 0) return received;
    if (!bexos_journal_request_valid(request, received, JOURNAL_ARCH)) return ERR_NOT_VALID;
    uint32_t status = transact(request, response);
    if (status != BEXOS_JOURNAL_OK) {
        /* An error response contains no partially read or uncommitted state. */
        memset(response, 0, sizeof(response));
        memcpy(response, "BEXJR001", 8);
    }
    bexos_journal_status(response, status);
    int sent = tipc_send1(channel, response, sizeof(response));
    return sent < 0 ? sent : NO_ERROR;
}

int bexos_journal_add_service(struct tipc_hset* hset) {
    static const struct uuid kernel = UUID_KERNEL_VALUE;
    static const struct uuid* allowed[] = { &kernel };
    static const struct tipc_port_acl acl = {
        .flags = IPC_PORT_ALLOW_TA_CONNECT, .uuid_num = 1, .uuids = allowed,
    };
    static const struct tipc_port port = {
        .name = BEXOS_JOURNAL_PORT, .msg_max_size = BEXOS_JOURNAL_BYTES,
        .msg_queue_len = 1, .acl = &acl,
    };
    static const struct tipc_srv_ops ops = { .on_message = on_message };
    int rc = tipc_add_service(hset, &port, 1, 1, &ops);
    extern int bexos_boot_selection_add_service(struct tipc_hset* hset);
    if (!rc) rc = bexos_boot_selection_add_service(hset);
    return rc;
}
