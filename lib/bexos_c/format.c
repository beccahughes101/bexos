/* Allocation-free C formatting. The pinned formatter is licensed under 0BSD. */
#define NANOPRINTF_USE_FIELD_WIDTH_FORMAT_SPECIFIERS 1
#define NANOPRINTF_USE_PRECISION_FORMAT_SPECIFIERS 1
#define NANOPRINTF_USE_FLOAT_FORMAT_SPECIFIERS 1
#define NANOPRINTF_USE_LARGE_FORMAT_SPECIFIERS 1
#define NANOPRINTF_USE_SMALL_FORMAT_SPECIFIERS 1
#define NANOPRINTF_USE_BINARY_FORMAT_SPECIFIERS 1
#define NANOPRINTF_USE_WRITEBACK_FORMAT_SPECIFIERS 1
#define NANOPRINTF_USE_ALT_FORM_FLAG 1
#define NANOPRINTF_USE_FLOAT_HEX_FORMAT_SPECIFIER 1
#define NANOPRINTF_USE_FLOAT_SCI_FORMAT_SPECIFIER 1
#define NANOPRINTF_USE_FLOAT_SHORTEST_FORMAT_SPECIFIER 1
#define NANOPRINTF_IMPLEMENTATION
#include "nanoprintf.h"
#include "format.h"
#include <stdio.h>
#include <errno.h>

int vsnprintf(char *out, size_t size, const char *format, va_list args)
{
    if (!format || (!out && size)) { errno = EINVAL; return -1; }
    return npf_vsnprintf(out, size, format, args);
}
int snprintf(char *out, size_t size, const char *format, ...)
{
    va_list args;
    va_start(args, format);
    int result = vsnprintf(out, size, format, args);
    va_end(args);
    return result;
}
int bexos_format_output(void (*write_char)(int, void *), void *context, const char *format, va_list args)
{
    return npf_vpprintf(write_char, context, format, args);
}
