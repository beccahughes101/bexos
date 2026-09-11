#ifndef BEXOS_BOOT_SELECTION_H
#define BEXOS_BOOT_SELECTION_H
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#define BEXOS_BOOT_SELECTION_PORT "com.bexos.orchestrator.boot-selection"
#define BEXOS_BOOT_REQUEST_BYTES 256u
#define BEXOS_BOOT_STATE_BYTES 512u
#define BEXOS_BOOT_MAX_ATTEMPTS 2u
enum {
    BEXOS_BOOT_QUERY = 1,
    BEXOS_BOOT_STAGE = 2,
    BEXOS_BOOT_ATTEMPT = 3,
    BEXOS_BOOT_COMMIT = 4,
    BEXOS_BOOT_ABORT = 5,
};
enum { BEXOS_BOOT_IDLE = 0, BEXOS_BOOT_PENDING = 1, BEXOS_BOOT_TRIAL = 2 };

/* All integers are LE. Requests: magic[8], operation:u32, architecture:u32,
 * expected_revision:u64, component:u32, reserved:u32, identity[56], zero[168].
 * State: magic[8], status:u32, architecture:u32, revision:u64, phase:u32,
 * component:u32, committed_trusty[56], committed_monitor[56], pending[56],
 * attempts:u32, zero[52], last_successful_request[256].
 * Identity: slot:u32, zero:u32, generation:u64, image_sha256[32], length:u64.
 * The last exact request makes lost replies idempotent. Only TP storage
 * authenticates records; neither the wire magic nor revision provides trust.
 */
bool bexos_boot_request_valid(const uint8_t* request, size_t length);
bool bexos_boot_state_valid(const uint8_t* state);
bool bexos_boot_initial(uint8_t* state, const uint8_t* trusty_v1,
                       const uint8_t* monitor_v1);
uint32_t bexos_boot_next(const uint8_t* current, const uint8_t* request,
                        uint8_t* next);
#endif
