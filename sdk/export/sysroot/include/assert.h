#ifndef BEXOS_ASSERT_H
#define BEXOS_ASSERT_H
#define assert(expression) ((expression) ? (void)0 : __builtin_trap())
#endif
