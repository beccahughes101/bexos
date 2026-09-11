/* Runs on both guest ABIs: especially important for floating-point va_list. */
#include <stdio.h>
#include <string.h>
#include <stdint.h>
#include <limits.h>
#include <stdlib.h>
#include <stddef.h>
#include <sys/stat.h>
#include <errno.h>
#include <pthread.h>
#if defined(__aarch64__)
_Static_assert(sizeof(pthread_attr_t) == 64, "AArch64 pthread attribute ABI");
#elif defined(__x86_64__)
_Static_assert(sizeof(pthread_attr_t) == 56, "x86-64 pthread attribute ABI");
#endif
static void *stack_worker(void *argument)
{
    pthread_attr_t attr;
    void *base = NULL;
    size_t size = 0;
    volatile unsigned char local = 0;
    if (pthread_getattr_np(pthread_self(), &attr) ||
        pthread_attr_getstack(&attr, &base, &size) ||
        pthread_attr_destroy(&attr)) return (void *)1;
    if (size != (uintptr_t)argument || (uintptr_t)&local < (uintptr_t)base ||
        (uintptr_t)&local >= (uintptr_t)base + size) return (void *)2;
    return NULL;
}
/* Rust's LinuxStat64 and both GNU header names must agree on the metadata
 * layout used by Mesa's file-reading helper. */
_Static_assert(sizeof(struct stat) == sizeof(struct stat64), "stat ABI size");
_Static_assert(offsetof(struct stat, st_size) == 48, "stat size offset");
#if defined(__aarch64__)
_Static_assert(sizeof(struct stat) == 128, "AArch64 stat ABI");
#elif defined(__x86_64__)
_Static_assert(sizeof(struct stat) == 144, "x86-64 stat ABI");
#endif
int bexos_c_format_probe(void)
{
    char buffer[128];
    int n = snprintf(buffer, sizeof(buffer), "%lld %llu %08x %.3f %.*s", LLONG_MIN, ULLONG_MAX, 0x1234, 1.25, 3, "abcdef");
    const char *expected = "-9223372036854775808 18446744073709551615 00001234 1.250 abc";
    if (n != (int)strlen(expected) || strcmp(buffer, expected)) return 1;
    char small[5] = {1, 1, 1, 1, 99};
    if (snprintf(small, 4, "%s", "abcdef") != 6 || memcmp(small, "abc\0", 4) || small[4] != 99) return 2;
    if (snprintf(NULL, 0, "%s=%d", "length", 123) != 10) return 3;
    if (snprintf(buffer, sizeof(buffer), "%.2e %.3g", 123.0, 12.5) < 0 || strcmp(buffer, "1.23e+02 12.5")) return 4;
    void *memory[32];
    for (size_t i = 0; i < 32; i++) {
        memory[i] = malloc(i + 1);
        if (!memory[i] || (uintptr_t)memory[i] % 16) return 5;
    }
    for (size_t i = 0; i < 32; i++) free(memory[i]);
    struct stat metadata;
    memset(&metadata, 0xa5, sizeof(metadata));
    errno = 0;
    if (fstat(-1, &metadata) != -1 || errno != EBADF) return 6;
    for (size_t i = 0; i < sizeof(metadata); i++)
        if (((unsigned char *)&metadata)[i] != 0xa5) return 7;
    if (atoi(" \t-253.0") != -253 || atoi("25.3.0") != 25) return 8;
    struct { uintptr_t before; pthread_attr_t attr; uintptr_t after; } guarded = { .before = 17, .after = 29 };
    if (pthread_attr_init(&guarded.attr) ||
        pthread_attr_setstacksize(&guarded.attr, 512 * 1024)) return 9;
    pthread_t thread;
    if (pthread_create(&thread, &guarded.attr, stack_worker, (void *)(512 * 1024))) return 10;
    void *result = (void *)3;
    if (pthread_join(thread, &result) || result || pthread_attr_destroy(&guarded.attr)) return 11;
    if (guarded.before != 17 || guarded.after != 29) return 12;
    if (pthread_join(pthread_self(), NULL) != EDEADLK) return 13;
    if (pthread_join(thread, NULL) != ESRCH) return 14;
    return 0;
}
