#include "include/bexos_package_state.h"
#include <string.h>
uint32_t bexos_package_u32(const uint8_t* p, size_t o) { uint32_t v = 0; for (size_t i = 0; i < 4; i++) v |= (uint32_t)p[o+i] << (i*8); return v; }
uint64_t bexos_package_u64(const uint8_t* p, size_t o) { uint64_t v = 0; for (size_t i = 0; i < 8; i++) v |= (uint64_t)p[o+i] << (i*8); return v; }
void bexos_package_put32(uint8_t* p, size_t o, uint32_t v) { for (size_t i = 0; i < 4; i++) p[o+i] = (uint8_t)(v >> (i*8)); }
void bexos_package_put64(uint8_t* p, size_t o, uint64_t v) { for (size_t i = 0; i < 8; i++) p[o+i] = (uint8_t)(v >> (i*8)); }
int bexos_package_valid(const uint8_t* p, size_t n) {
    return n >= BEXOS_PACKAGE_HEADER && n <= BEXOS_PACKAGE_MAX && !memcmp(p, "PKGSEC01", 8)
        && bexos_package_u32(p, 88) == n - BEXOS_PACKAGE_HEADER && !bexos_package_u32(p, 92)
        && (bexos_package_u32(p, 8) == 1 || bexos_package_u32(p, 8) == 2);
}
int bexos_package_next(const uint8_t* old, size_t old_n, const uint8_t* request, size_t n, uint8_t* next) {
    if (!bexos_package_valid(request, n) || bexos_package_u32(request, 8) != 2 || bexos_package_u32(request, 12)) return 4;
    uint64_t revision = bexos_package_u64(request, 48);
    if (revision == UINT64_MAX) return 4;
    if (old_n) {
        if (!bexos_package_valid(old, old_n) || memcmp(old+16, request+16, 32)) return 4;
        if (bexos_package_u64(old, 48) != revision) return 2;
        /* Root is the epoch for online-role versions. Only pkgd can call this
         * endpoint; it resets those versions after authenticating rotated keys.
         * Root and validated time never decrease, including across epochs. */
        int rotated_root = bexos_package_u64(request, 56) > bexos_package_u64(old, 56);
        for (size_t offset = 56; offset <= 80; offset += 8) {
            if (rotated_root && (offset == 64 || offset == 72)) continue;
            if (bexos_package_u64(request, offset) < bexos_package_u64(old, offset)) return 2;
        }
    } else if (revision) return 2;
    memcpy(next, request, n);
    bexos_package_put64(next, 48, revision+1);
    return 0;
}
