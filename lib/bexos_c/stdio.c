/* Unbuffered standard streams backed by the process's real granted libc FDs.
 * Mesa file and Unix syslog sinks are disabled in the BexOS build. */
#include <stdio.h>
#include <unistd.h>
#include <errno.h>
#include <pthread.h>
#include <string.h>
#include "format.h"

static FILE input_stream = { ._fileno = 0 };
static FILE output_stream = { ._fileno = 1 };
static FILE error_stream = { ._fileno = 2 };
FILE *stdin = &input_stream;
FILE *stdout = &output_stream;
FILE *stderr = &error_stream;
static pthread_mutex_t output_mutex = PTHREAD_MUTEX_INITIALIZER;
struct output { int fd, error; size_t count; unsigned char bytes[512]; };
static void flush(struct output *out)
{
    size_t offset = 0;
    while (!out->error && offset < out->count) {
        ssize_t written = write(out->fd, out->bytes + offset, out->count - offset);
        if (written < 0 && errno == EINTR) continue;
        if (written <= 0) { out->error = errno ? errno : EIO; break; }
        offset += written;
    }
    out->count = 0;
}
static void output_char(int c, void *context)
{
    struct output *out = context;
    if (out->error) return;
    out->bytes[out->count++] = c;
    if (out->count == sizeof(out->bytes)) flush(out);
}
int vfprintf(FILE *stream, const char *format, va_list args)
{
    if ((stream != stdout && stream != stderr) || !format) { errno = EINVAL; return -1; }
    int status = pthread_mutex_lock(&output_mutex);
    if (status) { errno = status; return -1; }
    struct output out = { .fd = stream->_fileno };
    int count = bexos_format_output(output_char, &out, format, args);
    flush(&out);
    pthread_mutex_unlock(&output_mutex);
    if (out.error) { errno = out.error; return -1; }
    return count;
}
int fprintf(FILE *stream, const char *format, ...)
{
    va_list args;
    va_start(args, format);
    int result = vfprintf(stream, format, args);
    va_end(args);
    return result;
}
int fflush(FILE *stream)
{
    if (!stream || stream == stdout || stream == stderr || stream == stdin) return 0;
    errno = EBADF;
    return EOF;
}
