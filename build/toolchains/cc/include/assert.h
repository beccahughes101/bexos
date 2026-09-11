#ifndef BEXOS_CC_ASSERT_H
#define BEXOS_CC_ASSERT_H
#if !defined(__cplusplus) && !defined(static_assert)
#define static_assert _Static_assert
#endif
#ifdef NDEBUG
#define assert(expr) ((void)0)
#else
#define assert(expr) ((expr) ? (void)0 : __builtin_trap())
#endif
#endif
