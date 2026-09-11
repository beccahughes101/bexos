#include "include/bexos_journal.h"
#undef NDEBUG
#include <assert.h>
#include <string.h>

int main(void) {
    uint8_t old[BEXOS_JOURNAL_BYTES], request[BEXOS_JOURNAL_BYTES], next[BEXOS_JOURNAL_BYTES];
    for (uint32_t arch = 1; arch <= 2; ++arch) {
        for (uint32_t component = 1; component <= arch; ++component) {
            bexos_journal_initial(old, arch, component);
            assert(bexos_journal_record_valid(old, arch, component));
            memcpy(request, old, sizeof(old));
            request[8] = BEXOS_JOURNAL_COMMIT;
            request[20] = 2;
            request[24] = 7;
            request[32] = 1;
            memset(request+40, 0x5a, 32);
            assert(bexos_journal_request_valid(request, sizeof(request), arch));
            assert(bexos_journal_next(old, request, next) == BEXOS_JOURNAL_OK);
            assert(bexos_journal_generation(next) == 7);
            assert(bexos_journal_record_valid(next, arch, component));
            memcpy(old, next, sizeof(old));
            /* Resolve an uncertain commit through the identical durable record. */
            assert(bexos_journal_next(old, request, next) == BEXOS_JOURNAL_OK);
            assert(!memcmp(old, next, sizeof(old)));
            request[40] ^= 1;
            assert(bexos_journal_next(old, request, next) == BEXOS_JOURNAL_ROLLBACK);
            request[40] ^= 1;
            request[24] = 8;
            assert(bexos_journal_next(old, request, next) == BEXOS_JOURNAL_ROLLBACK);
            request[32] = 7;
            assert(bexos_journal_next(old, request, next) == BEXOS_JOURNAL_ROLLBACK);
            request[20] = 1;
            assert(bexos_journal_next(old, request, next) == BEXOS_JOURNAL_OK);
            for (size_t length = 0; length < sizeof(request); ++length)
                assert(!bexos_journal_request_valid(request, length, arch));
            for (size_t i = 72; i < sizeof(request); ++i) {
                request[i] = 1;
                assert(!bexos_journal_request_valid(request, sizeof(request), arch));
                request[i] = 0;
            }
            assert(!bexos_journal_request_valid(request, sizeof(request), 3-arch));
            memset(request+40, 0, 32);
            assert(!bexos_journal_request_valid(request, sizeof(request), arch));
        }
    }
    return 0;
}
