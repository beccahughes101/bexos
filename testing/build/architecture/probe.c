#if defined(__x86_64__)
/* Compiler intrinsics must work without importing a hosted libc sysroot. */
#include <immintrin.h>
#endif
unsigned long architecture_c_probe(void) {
#if defined(__aarch64__)
    return 183;
#elif defined(__x86_64__)
    return 62;
#else
#error Unexpected guest C compiler target
#endif
}
