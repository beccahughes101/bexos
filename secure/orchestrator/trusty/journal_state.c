#include "include/bexos_journal.h"
#include <string.h>

uint32_t bexos_journal_word(const uint8_t* b, size_t off) {
    return (uint32_t)b[off] | (uint32_t)b[off+1] << 8 |
           (uint32_t)b[off+2] << 16 | (uint32_t)b[off+3] << 24;
}
static uint64_t wide(const uint8_t* b, size_t off) {
    return (uint64_t)bexos_journal_word(b, off) | (uint64_t)bexos_journal_word(b, off+4) << 32;
}
uint64_t bexos_journal_generation(const uint8_t* b) { return wide(b, 24); }
static bool zero(const uint8_t* b, size_t n) {
    for (size_t i = 0; i < n; ++i) if (b[i]) return false;
    return true;
}
void bexos_journal_status(uint8_t* b, uint32_t status) {
    for (size_t i = 0; i < 4; ++i) b[8+i] = status >> (8*i);
}
static bool header(const uint8_t* b, uint32_t arch, uint32_t component) {
    return !memcmp(b, "BEXJR001", 8) && (arch == 1 || arch == 2) &&
        (component == 1 || (arch == 2 && component == 2)) &&
        bexos_journal_word(b, 12) == arch && bexos_journal_word(b, 16) == component &&
        zero(b+72, 24);
}
bool bexos_journal_request_valid(const uint8_t* b, size_t length, uint32_t arch) {
    if (length != BEXOS_JOURNAL_BYTES || !header(b, arch, bexos_journal_word(b, 16))) return false;
    uint32_t command = bexos_journal_word(b, 8);
    if (command == BEXOS_JOURNAL_QUERY) return zero(b+20, 52);
    return command == BEXOS_JOURNAL_COMMIT &&
        bexos_journal_word(b, 20) >= 1 && bexos_journal_word(b, 20) <= 2 &&
        wide(b, 32) >= 1 && wide(b, 24) > wide(b, 32) && !zero(b+40, 32);
}
bool bexos_journal_record_valid(const uint8_t* b, uint32_t arch, uint32_t component) {
    if (!header(b, arch, component) || bexos_journal_word(b, 8) != 0) return false;
    if (wide(b, 24) == 1) return bexos_journal_word(b, 20) == 1 && zero(b+32, 40);
    return wide(b, 24) > 1 && wide(b, 32) >= 1 && wide(b, 32) < wide(b, 24) &&
        (bexos_journal_word(b, 20) == 1 || bexos_journal_word(b, 20) == 2) && !zero(b+40, 32);
}
void bexos_journal_initial(uint8_t* b, uint32_t arch, uint32_t component) {
    memset(b, 0, BEXOS_JOURNAL_BYTES);
    memcpy(b, "BEXJR001", 8);
    b[12] = arch;
    b[16] = component;
    b[20] = 1;
    b[24] = 1;
}
uint32_t bexos_journal_next(const uint8_t* current, const uint8_t* request, uint8_t* next) {
    uint32_t arch = bexos_journal_word(current, 12), component = bexos_journal_word(current, 16);
    if (!bexos_journal_record_valid(current, arch, component) ||
        !bexos_journal_request_valid(request, BEXOS_JOURNAL_BYTES, arch) ||
        bexos_journal_word(request, 16) != component ||
        bexos_journal_word(request, 8) != BEXOS_JOURNAL_COMMIT) return BEXOS_JOURNAL_INVALID;
    /* A repeated exact commit resolves a lost acknowledgement without a
     * second durable write. A different image at that generation is rejected. */
    bool repeated = !memcmp(current+12, request+12, BEXOS_JOURNAL_BYTES-12);
    if (!repeated && (wide(request, 32) != wide(current, 24) ||
        wide(request, 24) <= wide(current, 24) ||
        bexos_journal_word(request, 20) == bexos_journal_word(current, 20))) return BEXOS_JOURNAL_ROLLBACK;
    memcpy(next, request, BEXOS_JOURNAL_BYTES);
    bexos_journal_status(next, BEXOS_JOURNAL_OK);
    return BEXOS_JOURNAL_OK;
}
