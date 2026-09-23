#ifndef BEXOS_PACKAGE_STATE_H
#define BEXOS_PACKAGE_STATE_H
#include <stddef.h>
#include <stdint.h>
#define BEXOS_PACKAGE_STATE_PORT "com.bexos.package-state"
#define BEXOS_PACKAGE_HEADER 96u
#define BEXOS_PACKAGE_MAX 3168u
uint32_t bexos_package_u32(const uint8_t*, size_t);
uint64_t bexos_package_u64(const uint8_t*, size_t);
void bexos_package_put32(uint8_t*, size_t, uint32_t);
void bexos_package_put64(uint8_t*, size_t, uint64_t);
int bexos_package_valid(const uint8_t*, size_t);
int bexos_package_next(const uint8_t*, size_t, const uint8_t*, size_t, uint8_t*);
#endif
