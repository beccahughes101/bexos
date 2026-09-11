#ifndef BEXOS_JOURNAL_H
#define BEXOS_JOURNAL_H
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#define BEXOS_JOURNAL_PORT "com.bexos.orchestrator.replacement"
#define BEXOS_JOURNAL_BYTES 96u
enum bexos_journal_status {
    BEXOS_JOURNAL_OK = 0,
    BEXOS_JOURNAL_INVALID = 1,
    BEXOS_JOURNAL_ROLLBACK = 2,
    BEXOS_JOURNAL_UNAVAILABLE = 3,
    BEXOS_JOURNAL_UNCERTAIN = 4,
};
enum { BEXOS_JOURNAL_QUERY = 1, BEXOS_JOURNAL_COMMIT = 2 };

/* Canonical LE record: magic[8], command/status:u32, architecture:u32,
 * component:u32, slot:u32, generation:u64, previous:u64, image_hash[32],
 * reserved[24]. Architecture 1=ARM, 2=x86; component 1=Trusty, 2=monitor.
 * A committed record stores status=0. Only protected storage authenticates it.
 */
uint32_t bexos_journal_word(const uint8_t* bytes, size_t offset);
uint64_t bexos_journal_generation(const uint8_t* bytes);
void bexos_journal_status(uint8_t* bytes, uint32_t status);
bool bexos_journal_request_valid(const uint8_t* bytes, size_t length, uint32_t architecture);
bool bexos_journal_record_valid(const uint8_t* bytes, uint32_t architecture, uint32_t component);
void bexos_journal_initial(uint8_t* bytes, uint32_t architecture, uint32_t component);
uint32_t bexos_journal_next(const uint8_t* current, const uint8_t* request, uint8_t* next);
#endif
