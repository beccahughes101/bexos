#include "include/bexos_boot_selection.h"
#include "include/bexos_journal.h"
#include <string.h>

static uint32_t word(const uint8_t* b, size_t off) {
    return bexos_journal_word(b, off);
}
static uint64_t wide(const uint8_t* b, size_t off) {
    return word(b, off) | (uint64_t)word(b, off+4) << 32;
}
static void put(uint8_t* b, size_t off, uint64_t value, size_t bytes) {
    for (size_t i = 0; i < bytes; ++i) b[off+i] = value >> (i*8);
}
static bool zero(const uint8_t* b, size_t n) {
    for (size_t i = 0; i < n; ++i) if (b[i]) return false;
    return true;
}
static bool identity(const uint8_t* b, bool initial) {
    uint64_t generation = wide(b, 8), length = wide(b, 48);
    if (!zero(b+4, 4) || word(b, 0) < 1 || word(b, 0) > 2) return false;
    if (initial && generation == 1)
        return word(b, 0) == 1 && zero(b+16, 40);
    return generation > 1 && length >= 64 && length <= 64u*1024u*1024u &&
        !zero(b+16, 32);
}
static bool header(const uint8_t* b) {
    return !memcmp(b, "BEXBS002", 8) && (word(b, 12) == 1 || word(b, 12) == 2);
}
bool bexos_boot_request_valid(const uint8_t* r, size_t length) {
    if (length != BEXOS_BOOT_REQUEST_BYTES || !header(r) ||
        !zero(r+28, 4) || !zero(r+88, 168)) return false;
    uint32_t op = word(r, 8);
    if (op == BEXOS_BOOT_QUERY) return zero(r+16, 72);
    if (op < BEXOS_BOOT_STAGE || op > BEXOS_BOOT_ABORT ||
        wide(r, 16) == 0 || wide(r, 16) == UINT64_MAX ||
        word(r, 24) < 1 || word(r, 24) > 2) return false;
    if (word(r, 12) == 1 && word(r, 24) != 1) return false;
    return identity(r+32, false);
}
bool bexos_boot_state_valid(const uint8_t* s) {
    if (!header(s) || word(s, 8) || wide(s, 16) == 0 ||
        !identity(s+32, true) || !identity(s+88, true) ||
        !zero(s+204, 52)) return false;
    if (word(s, 12) == 1 &&
        (word(s+88, 0) != 1 || wide(s+88, 8) != 1 || !zero(s+104, 40))) return false;
    uint32_t phase = word(s, 24), component = word(s, 28), attempts = word(s, 200);
    if (phase == BEXOS_BOOT_IDLE) {
        if (component || !zero(s+144, 60)) return false;
    } else {
        if ((word(s, 12) == 1 && component != 1) ||
            phase > BEXOS_BOOT_TRIAL || component < 1 || component > 2 ||
            !identity(s+144, false) || attempts > BEXOS_BOOT_MAX_ATTEMPTS ||
            (phase == BEXOS_BOOT_PENDING && attempts) ||
            (phase == BEXOS_BOOT_TRIAL && !attempts)) return false;
        const uint8_t* committed = s+32+(component-1)*56;
        if (word(committed, 0) == word(s+144, 0) ||
            wide(committed, 8) >= wide(s+144, 8)) return false;
    }
    if (wide(s, 16) == 1)
        return zero(s+256, 256) && phase == BEXOS_BOOT_IDLE &&
            wide(s+32, 8) == 1 && wide(s+88, 8) == 1;
    const uint8_t* r = s+256;
    if (!bexos_boot_request_valid(r, 256) || word(r, 8) == BEXOS_BOOT_QUERY ||
        word(r, 12) != word(s, 12) ||
        wide(r, 16) != wide(s, 16)-1) return false;
    /* Validate the result as well as the request before acknowledging it. */
    const uint8_t* active = s+32+(word(r, 24)-1)*56;
    switch (word(r, 8)) {
    case BEXOS_BOOT_STAGE:
        return phase == BEXOS_BOOT_PENDING && component == word(r, 24) &&
            !memcmp(s+144, r+32, 56);
    case BEXOS_BOOT_ATTEMPT:
        return phase == BEXOS_BOOT_TRIAL && component == word(r, 24) &&
            !memcmp(s+144, r+32, 56);
    case BEXOS_BOOT_COMMIT:
        return phase == BEXOS_BOOT_IDLE && !memcmp(active, r+32, 56);
    case BEXOS_BOOT_ABORT:
        return phase == BEXOS_BOOT_IDLE && wide(active, 8) < wide(r+32, 8) &&
            word(active, 0) != word(r+32, 0);
    default:
        return false;
    }
}
bool bexos_boot_initial(uint8_t* s, const uint8_t* trusty, const uint8_t* monitor) {
    /* A v1 generation above one requires its real persisted image length.
     * Do not invent it or silently reset the protected floor during migration. */
    unsigned arch = word(trusty, 12);
    if (!bexos_journal_record_valid(trusty, arch, 1) ||
        bexos_journal_generation(trusty) != 1) return false;
    /* ARM has no replaceable hypervisor component. Its reserved identity is
     * always INITIAL and there is no compatibility journal to manufacture. */
    if (arch == 2 && (!monitor || !bexos_journal_record_valid(monitor, arch, 2) ||
        bexos_journal_generation(monitor) != 1)) return false;
    if (arch == 1 && monitor) return false;
    memset(s, 0, BEXOS_BOOT_STATE_BYTES);
    memcpy(s, "BEXBS002", 8);
    s[12] = arch; s[16] = 1;
    s[32] = 1; s[40] = 1; s[88] = 1; s[96] = 1;
    return true;
}
uint32_t bexos_boot_next(const uint8_t* s, const uint8_t* r, uint8_t* next) {
    if (!bexos_boot_state_valid(s) || !bexos_boot_request_valid(r, 256) ||
        word(s, 12) != word(r, 12))
        return BEXOS_JOURNAL_INVALID;
    uint32_t op = word(r, 8), component = word(r, 24), phase = word(s, 24);
    if (op == BEXOS_BOOT_QUERY || !memcmp(r, s+256, 256)) {
        memcpy(next, s, 512);
        return BEXOS_JOURNAL_OK;
    }
    if (wide(r, 16) != wide(s, 16)) return BEXOS_JOURNAL_ROLLBACK;
    const uint8_t* committed = s+32+(component-1)*56;
    if (op == BEXOS_BOOT_STAGE) {
        if (phase != BEXOS_BOOT_IDLE || word(r+32, 0) == word(committed, 0) ||
            wide(r+32, 8) <= wide(committed, 8)) return BEXOS_JOURNAL_ROLLBACK;
    } else {
        if (phase == BEXOS_BOOT_IDLE || word(s, 28) != component ||
            memcmp(r+32, s+144, 56)) return BEXOS_JOURNAL_ROLLBACK;
        if (op == BEXOS_BOOT_ATTEMPT && word(s, 200) >= BEXOS_BOOT_MAX_ATTEMPTS)
            return BEXOS_JOURNAL_ROLLBACK;
        if (op == BEXOS_BOOT_COMMIT && phase != BEXOS_BOOT_TRIAL)
            return BEXOS_JOURNAL_INVALID;
    }
    memcpy(next, s, 512);
    if (op == BEXOS_BOOT_STAGE) {
        put(next, 24, BEXOS_BOOT_PENDING, 4);
        put(next, 28, component, 4);
        memcpy(next+144, r+32, 56);
    } else if (op == BEXOS_BOOT_ATTEMPT) {
        put(next, 24, BEXOS_BOOT_TRIAL, 4);
        put(next, 200, word(s, 200)+1, 4);
    } else {
        if (op == BEXOS_BOOT_COMMIT) memcpy(next+32+(component-1)*56, s+144, 56);
        memset(next+24, 0, 8);
        memset(next+144, 0, 60);
    }
    put(next, 16, wide(s, 16)+1, 8);
    memcpy(next+256, r, 256);
    return BEXOS_JOURNAL_OK;
}
