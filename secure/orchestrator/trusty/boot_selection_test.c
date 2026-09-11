#include "include/bexos_boot_selection.h"
#include "include/bexos_journal.h"
#undef NDEBUG
#include <assert.h>
#include <string.h>

static void request(uint8_t* r, const uint8_t* state, unsigned op, unsigned component) {
    memset(r, 0, 256);
    memcpy(r, "BEXBS002", 8);
    r[8] = op; r[12] = 2;
    if (op == BEXOS_BOOT_QUERY) return;
    memcpy(r+16, state+16, 8); r[24] = component;
    if (op == BEXOS_BOOT_STAGE) {
        r[32] = 2; r[40] = 2; memset(r+48, 0x5a, 32); r[81] = 1;
    } else memcpy(r+32, state+144, 56);
}
static void initial(uint8_t* state) {
    uint8_t trusty[96], monitor[96];
    bexos_journal_initial(trusty, 2, 1);
    bexos_journal_initial(monitor, 2, 2);
    assert(bexos_boot_initial(state, trusty, monitor));
    assert(bexos_boot_state_valid(state));
    monitor[20] = 2; monitor[24] = 7; monitor[32] = 1; memset(monitor+40, 1, 32);
    assert(!bexos_boot_initial(state, trusty, monitor));
}
static void apply(uint8_t* state, uint8_t* r) {
    uint8_t next[512], retry[512];
    assert(bexos_boot_next(state, r, next) == BEXOS_JOURNAL_OK);
    assert(bexos_boot_state_valid(next));
    assert(bexos_boot_next(next, r, retry) == BEXOS_JOURNAL_OK);
    assert(!memcmp(next, retry, 512));
    memcpy(state, next, 512);
}
int main(void) {
    uint8_t state[512], r[256], other[256], next[512], old[512];
    for (unsigned component = 1; component <= 2; ++component) {
        initial(state);
        request(r, state, BEXOS_BOOT_STAGE, component);
        for (size_t length = 0; length < 256; ++length)
            assert(!bexos_boot_request_valid(r, length));
        for (size_t at = 88; at < 256; ++at) {
            r[at] = 1; assert(!bexos_boot_request_valid(r, 256)); r[at] = 0;
        }
        apply(state, r);
        assert(state[40] == 1 && state[96] == 1);
        state[160] ^= 1;
        assert(!bexos_boot_state_valid(state));
        state[160] ^= 1;
        request(other, state, BEXOS_BOOT_STAGE, 3-component);
        assert(bexos_boot_next(state, other, next) == BEXOS_JOURNAL_ROLLBACK);
        request(other, state, BEXOS_BOOT_COMMIT, component);
        assert(bexos_boot_next(state, other, next) == BEXOS_JOURNAL_INVALID);
        request(r, state, BEXOS_BOOT_ATTEMPT, component);
        apply(state, r);
        assert(state[200] == 1);
        memcpy(old, state, 512);
        request(r, state, BEXOS_BOOT_COMMIT, component);
        apply(state, r);
        assert(state[24] == BEXOS_BOOT_IDLE && state[200] == 0);
        assert(state[40+(component-1)*56] == 2);
        state[48+(component-1)*56] ^= 1;
        assert(!bexos_boot_state_valid(state));
        state[48+(component-1)*56] ^= 1;
        request(other, old, BEXOS_BOOT_ABORT, component);
        memset(next, 0xa5, sizeof(next));
        assert(bexos_boot_next(state, other, next) == BEXOS_JOURNAL_ROLLBACK);
        for (size_t i = 0; i < sizeof(next); ++i) assert(next[i] == 0xa5);
        r[48] ^= 1; /* same revision, conflicting digest cannot be an exact retry */
        assert(bexos_boot_next(state, r, next) == BEXOS_JOURNAL_ROLLBACK);

        initial(state);
        request(r, state, BEXOS_BOOT_STAGE, component); apply(state, r);
        for (unsigned trial = 1; trial <= BEXOS_BOOT_MAX_ATTEMPTS; ++trial) {
            request(r, state, BEXOS_BOOT_ATTEMPT, component); apply(state, r);
            assert(state[200] == trial);
        }
        request(r, state, BEXOS_BOOT_ATTEMPT, component);
        assert(bexos_boot_next(state, r, next) == BEXOS_JOURNAL_ROLLBACK);
        request(r, state, BEXOS_BOOT_ABORT, component); apply(state, r);
        assert(state[40] == 1 && state[96] == 1);
        assert(state[24] == BEXOS_BOOT_IDLE);
        request(r, state, BEXOS_BOOT_QUERY, 0); apply(state, r);
        memcpy(old, state, 512);
        for (size_t i = 204; i < 256; ++i) {
            state[i] = 1; assert(!bexos_boot_state_valid(state)); state[i] = 0;
        }
        assert(!memcmp(old, state, 512));
    }
    return 0;
}
