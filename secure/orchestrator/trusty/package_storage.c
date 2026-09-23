/* Package state hashes and secrets live in authenticated encrypted TP storage.
 * No normal-world fallback, repair, or reset operation exists. */
#include "include/bexos_package_state.h"
#include <lib/storage/storage.h>
#include <lib/tipc/tipc_srv.h>
#include <string.h>
#include <uapi/err.h>
static uint8_t request[BEXOS_PACKAGE_MAX], response[BEXOS_PACKAGE_MAX], previous[BEXOS_PACKAGE_MAX];
static void name_for_key(const uint8_t* key, char* out) {
    static const char alphabet[] = "abcdefghijklmnopqrstuvwxyz234567";
    memcpy(out, "pkg-", 4);
    uint32_t bits = 0; unsigned count = 0; size_t pos = 4;
    for (size_t i = 0; i < 32; i++) {
        bits = (bits << 8) | key[i]; count += 8;
        while (count >= 5) { count -= 5; out[pos++] = alphabet[(bits >> count) & 31]; }
    }
    if (count) out[pos++] = alphabet[(bits << (5-count)) & 31];
    out[pos] = 0;
}
static int on_package_message(const struct tipc_port* port, handle_t channel, void* context) {
    (void)port; (void)context;
    int length = tipc_recv1(channel, BEXOS_PACKAGE_HEADER, request, sizeof(request));
    if (length < 0) return length;
    uint32_t status = 3;
    size_t response_length = BEXOS_PACKAGE_HEADER;
    storage_session_t session;
    file_handle_t file = 0;
    bool session_open = false, file_open = false;
    memset(response, 0, sizeof(response)); memset(previous, 0, sizeof(previous));
    memcpy(response, "PKGSEC01", 8);
    if (!bexos_package_valid(request, (size_t)length)) { status = 4; goto done; }
    char name[57]; name_for_key(request+16, name);
    if (storage_open_session(&session, STORAGE_CLIENT_TP_PORT) < 0) goto done;
    session_open = true;
    int rc = storage_open_file(session, &file, name, 0, 0);
    size_t old_length = 0;
    if (rc >= 0) {
        file_open = true;
        storage_off_t size;
        if (storage_get_file_size(file, &size) < 0 || size < BEXOS_PACKAGE_HEADER || size > BEXOS_PACKAGE_MAX) goto done;
        old_length = (size_t)size;
        if (storage_read(file, 0, previous, old_length) != (int)old_length || !bexos_package_valid(previous, old_length)) goto done;
    } else if (rc != ERR_NOT_FOUND) goto done;
    if (bexos_package_u32(request, 8) == 1) {
        if (!old_length) { status = 1; goto done; }
        memcpy(response, previous, old_length); response_length = old_length; status = 0; goto done;
    }
    status = bexos_package_next(previous, old_length, request, (size_t)length, response);
    if (status) goto done;
    status = 3;
    if (!file_open) {
        if (storage_open_file(session, &file, name, STORAGE_FILE_OPEN_CREATE | STORAGE_FILE_OPEN_CREATE_EXCLUSIVE, 0) < 0) goto done;
        file_open = true;
    }
    if (storage_write(file, 0, response, (size_t)length, 0) != length || storage_set_file_size(file, (storage_off_t)length, 0) < 0 || storage_end_transaction(session, true) < 0) goto done;
    status = 0; response_length = (size_t)length;
done:
    if (file_open) storage_close_file(file);
    if (session_open) storage_close_session(session);
    if (status) { memset(response, 0, sizeof(response)); memcpy(response, "PKGSEC01", 8); response_length = BEXOS_PACKAGE_HEADER; }
    bexos_package_put32(response, 12, status);
    int sent = tipc_send1(channel, response, response_length);
    volatile uint8_t* secret = request; for (size_t i = 0; i < sizeof(request); i++) secret[i] = 0;
    secret = response; for (size_t i = 0; i < sizeof(response); i++) secret[i] = 0;
    secret = previous; for (size_t i = 0; i < sizeof(previous); i++) secret[i] = 0;
    return sent < 0 ? sent : NO_ERROR;
}
int bexos_package_add_service(struct tipc_hset* hset) {
    static const struct tipc_port_acl acl = { .flags = IPC_PORT_ALLOW_NS_CONNECT };
    static const struct tipc_port port = { .name = BEXOS_PACKAGE_STATE_PORT, .msg_max_size = BEXOS_PACKAGE_MAX, .msg_queue_len = 1, .acl = &acl };
    static const struct tipc_srv_ops ops = { .on_message = on_package_message };
    return tipc_add_service(hset, &port, 1, 1, &ops);
}
