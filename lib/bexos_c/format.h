#ifndef BEXOS_C_FORMAT_H
#define BEXOS_C_FORMAT_H
#include <stdarg.h>
int bexos_format_output(void (*write_char)(int, void *), void *context, const char *format, va_list args);
#endif
