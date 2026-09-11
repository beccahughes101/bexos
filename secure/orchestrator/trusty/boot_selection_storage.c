/* X86 boot trials use one TP transaction for both component selections. */
#if defined(__x86_64__)
#include "include/bexos_boot_selection.h"
#include "include/bexos_journal.h"
#include <lib/storage/storage.h>
#include <lib/tipc/tipc_srv.h>
#include <string.h>
#include <uapi/err.h>
#include <uapi/trusty_uuid.h>

static int legacy(storage_session_t session, const char* name, unsigned component,
                  uint8_t* record) {
    file_handle_t file;
    int rc = storage_open_file(session, &file, name, 0, 0);
    if (rc == ERR_NOT_FOUND) {
        bexos_journal_initial(record, 2, component);
        return 0;
    }
    if (rc < 0) return rc;
    storage_off_t size = 0;
    rc = storage_get_file_size(file, &size);
    if (rc == 0 && size == 96) rc = storage_read(file, 0, record, 96);
    storage_close_file(file);
    return rc == 96 ? 0 : ERR_NOT_VALID;
}
static uint32_t transact(const uint8_t* request, uint8_t* response) {
    storage_session_t session;
    file_handle_t file;
    bool opened = false;
    uint32_t status = BEXOS_JOURNAL_UNAVAILABLE;
    if (storage_open_session(&session, STORAGE_CLIENT_TP_PORT) < 0) return status;
    int rc = storage_open_file(session, &file, "replacement.boot.v2", 0, 0);
    if (rc == ERR_NOT_FOUND) {
        uint8_t trusty[96], monitor[96];
        if (legacy(session, "replacement.trusty.v1", 1, trusty) < 0 ||
            legacy(session, "replacement.monitor.v1", 2, monitor) < 0 ||
            !bexos_boot_initial(response, trusty, monitor)) goto done;
    } else if (rc < 0) goto done;
    else {
        opened = true;
        storage_off_t size;
        if (storage_get_file_size(file, &size) < 0 || size != 512 ||
            storage_read(file, 0, response, 512) != 512 ||
            !bexos_boot_state_valid(response)) goto done;
    }
    uint8_t next[512];
    status = bexos_boot_next(response, request, next);
    if (status != BEXOS_JOURNAL_OK || !memcmp(response, next, 512)) goto done;
    status = BEXOS_JOURNAL_UNAVAILABLE;
    if (!opened) {
        if (storage_open_file(session, &file, "replacement.boot.v2",
            STORAGE_FILE_OPEN_CREATE | STORAGE_FILE_OPEN_CREATE_EXCLUSIVE, 0) < 0) goto done;
        opened = true;
    }
    status = BEXOS_JOURNAL_UNCERTAIN;
    if (storage_write(file, 0, next, 512, 0) != 512) goto done;
    if (bexos_journal_word(request, 8) == BEXOS_BOOT_COMMIT) {
        unsigned component = bexos_journal_word(request, 24);
        const uint8_t* identity = next+32+(component-1)*56;
        uint8_t compatibility[96];
        bexos_journal_initial(compatibility, 2, component);
        memcpy(compatibility+20, identity, 4);
        memcpy(compatibility+24, identity+8, 8);
        memcpy(compatibility+32, response+40+(component-1)*56, 8);
        memcpy(compatibility+40, identity+16, 32);
        file_handle_t mirror;
        const char* name = component == 1 ? "replacement.trusty.v1" : "replacement.monitor.v1";
        if (storage_open_file(session, &mirror, name, STORAGE_FILE_OPEN_CREATE, 0) < 0) goto done;
        rc = storage_write(mirror, 0, compatibility, 96, 0);
        storage_close_file(mirror);
        if (rc != 96) goto done;
    }
    if (storage_end_transaction(session, true) < 0) goto done;
    memcpy(response, next, 512);
    status = BEXOS_JOURNAL_OK;
done:
    if (opened) storage_close_file(file);
    storage_close_session(session);
    return status;
}
static int on_message(const struct tipc_port* port, handle_t channel, void* context) {
    (void)port; (void)context;
    uint8_t request[256], response[512] = {0};
    int received = tipc_recv1(channel, sizeof(request), request, sizeof(request));
    if (received < 0) return received;
    if (!bexos_boot_request_valid(request, received)) return ERR_NOT_VALID;
    uint32_t status = transact(request, response);
    if (status != BEXOS_JOURNAL_OK) {
        memset(response, 0, 512);
        memcpy(response, "BEXBS002", 8);
    }
    bexos_journal_status(response, status);
    int sent = tipc_send1(channel, response, sizeof(response));
    return sent < 0 ? sent : NO_ERROR;
}
int bexos_boot_selection_add_service(struct tipc_hset* hset) {
    static const struct uuid kernel = UUID_KERNEL_VALUE;
    static const struct uuid* allowed[] = { &kernel };
    static const struct tipc_port_acl acl = {
        .flags = IPC_PORT_ALLOW_TA_CONNECT, .uuid_num = 1, .uuids = allowed,
    };
    static const struct tipc_port port = {
        .name = BEXOS_BOOT_SELECTION_PORT, .msg_max_size = 512,
        .msg_queue_len = 1, .acl = &acl,
    };
    static const struct tipc_srv_ops ops = { .on_message = on_message };
    return tipc_add_service(hset, &port, 1, 1, &ops);
}
#endif
