#ifndef BEXOS_CC_STRING_H
#define BEXOS_CC_STRING_H
#ifdef BEXOS_GNU_HEADERS
#include_next <string.h>
#else
typedef __SIZE_TYPE__ size_t;
void *memcpy(void *restrict dest, const void *restrict src, size_t n);
void *memmove(void *dest, const void *src, size_t n);
void *memset(void *dest, int c, size_t n);
int memcmp(const void *left, const void *right, size_t n);
#endif /* BEXOS_GNU_HEADERS */
#endif
