typedef __SIZE_TYPE__ size_t;
typedef __PTRDIFF_TYPE__ ssize_t;

extern int chmod(const char *path, unsigned int mode);
extern int clone(int (*function)(void *), void *stack, int flags, void *argument, ...);
extern int close(int fd);
extern void _exit(int status);
extern int fcntl(int fd, int command, ...);
extern int fchmod(int fd, unsigned int mode);
extern int fchown(int fd, unsigned int owner, unsigned int group);
extern int fork(void);
extern char *getcwd(char *buffer, size_t size);
extern int getgroups(int size, unsigned int list[]);
extern int getpid(void);
extern ssize_t getxattr(const char *path, const char *name, void *value, size_t size);
extern int kill(int pid, int signal);
extern int link(const char *source, const char *target);
extern ssize_t listxattr(const char *path, char *list, size_t size);
extern void *mmap(void *address, size_t length, int protection, int flags, int fd, long offset);
extern int mprotect(void *address, size_t length, int protection);
extern void *mremap(void *old_address, size_t old_size, size_t new_size, int flags, ...);
extern int msync(void *address, size_t length, int flags);
extern int munmap(void *address, size_t length);
extern int open(const char *path, int flags, ...);
extern int pipe(int fds[2]);
extern ssize_t pread(int fd, void *buffer, size_t count, long offset);
extern ssize_t read(int fd, void *buffer, size_t count);
extern ssize_t readlink(const char *path, char *buffer, size_t size);
extern int removexattr(const char *path, const char *name);
extern int setxattr(const char *path, const char *name, const void *value, size_t size, int flags);
extern int setgroups(size_t size, const unsigned int *list);
extern int setresgid(unsigned int real, unsigned int effective, unsigned int saved);
extern int setresuid(unsigned int real, unsigned int effective, unsigned int saved);
extern int sigpending(void *set);
extern int sigprocmask(int how, const void *set, void *old_set);
extern void (*signal(int number, void (*handler)(int)))(int);
extern int sigsuspend(const void *set);
extern int symlink(const char *target, const char *link_path);
extern long syscall(long number, ...);
extern int unlink(const char *path);
extern int waitpid(int pid, int *status, int options);
extern ssize_t write(int fd, const void *buffer, size_t count);

static int write_all(int fd, const char *bytes, size_t length) {
    while (length != 0) {
        ssize_t written = write(fd, bytes, length);
        if (written <= 0) {
            return -1;
        }
        bytes += written;
        length -= (size_t)written;
    }
    return 0;
}

static int same_bytes(const char *left, const char *right, size_t length) {
    for (size_t index = 0; index != length; ++index) {
        if (left[index] != right[index]) {
            return 0;
        }
    }
    return 1;
}

static int contains_bytes(const char *bytes, size_t length, const char *needle,
                          size_t needle_length) {
    if (needle_length > length) {
        return 0;
    }
    for (size_t offset = 0; offset <= length - needle_length; ++offset) {
        if (same_bytes(bytes + offset, needle, needle_length)) {
            return 1;
        }
    }
    return 0;
}

static int proc_maps_abi(void) {
    static const char path[] = "/proc/self/maps";
    static const char status_path[] = "/proc/thread-self/status";
    static const char stack_name[] = "[stack]";
    static const char image_name[] = "[image]";
    static const char identity[] = "Tgid:\t1\nPid:\t1\nPPid:\t0\n";
    char maps[4096];
    int fd = open(path, 0);
    if (fd < 0) {
        return 103;
    }
    ssize_t count = read(fd, maps, sizeof(maps));
    if (count <= 0 || close(fd) != 0
        || !contains_bytes(maps, (size_t)count, stack_name, sizeof(stack_name) - 1)
        || !contains_bytes(maps, (size_t)count, image_name, sizeof(image_name) - 1)) {
        return 104;
    }
    fd = open(status_path, 0);
    if (fd < 0) {
        return 105;
    }
    count = read(fd, maps, sizeof(maps));
    if (count <= 0 || close(fd) != 0
        || !contains_bytes(maps, (size_t)count, identity, sizeof(identity) - 1)) {
        return 106;
    }
    return 0;
}

static int filesystem_abi(void) {
    static const char base[] = "/data/rfc70-dynamic-base";
    static const char hard[] = "/data/rfc70-dynamic-hard";
    static const char symbolic[] = "/data/rfc70-dynamic-symbolic";
    static const char copied[] = "/data/rfc70-dynamic-copy";
    static const char relative_target[] = "rfc70-dynamic-base";
    static const char attribute[] = "user.rfc70";
    static const char attribute_value[] = "offline-abi";
    char buffer[64];
    char cwd[64];
    if (getcwd(cwd, sizeof(cwd)) != cwd || cwd[0] != '/') {
        return 50;
    }
    unlink(symbolic);
    unlink(hard);
    unlink(copied);
    unlink(base);

    int fd = open(base, 2 | 0x40 | 0x200 | 0x80000, 0600);
    if (fd < 0 || fcntl(fd, 1) != 1) {
        return 20;
    }
    if (fchmod(fd, 0640) != 0 || fchown(fd, (unsigned int)-1, (unsigned int)-1) != 0) {
        return 34;
    }
#if defined(__aarch64__)
    const long statfs_number = 43;
    const long fstatfs_number = 44;
    const long fadvise64_number = 223;
    const long statx_number = 291;
#else
    const long statfs_number = 137;
    const long fstatfs_number = 138;
    const long fadvise64_number = 221;
    const long statx_number = 332;
#endif
    unsigned long filesystem[15] = {0};
    if (syscall(statfs_number, "/data", filesystem) != 0 || filesystem[1] != 4096
        || filesystem[8] != 255) {
        return 42;
    }
    for (size_t index = 0; index != 15; ++index) {
        filesystem[index] = 0;
    }
    if (syscall(fstatfs_number, fd, filesystem) != 0 || filesystem[1] != 4096
        || filesystem[8] != 255) {
        return 43;
    }
    if (syscall(fadvise64_number, fd, 0L, 4096L, 2L) != 0) {
        return 44;
    }
    int duplicate = fcntl(fd, 1030, 3);
    if (duplicate < 3 || fcntl(duplicate, 1) != 1 || close(duplicate) != 0) {
        return 21;
    }
    if (write_all(fd, attribute_value, sizeof(attribute_value) - 1) != 0 || close(fd) != 0) {
        return 22;
    }
    union {
        unsigned long words[32];
        unsigned char bytes[256];
    } extended = {{0}};
    if (syscall(statx_number, -100L, base, 0L, 0xfffL, extended.bytes) != 0
        || (extended.words[0] & 0xffffffffUL) != 0xfffUL
        || ((extended.words[3] >> 32) & 0xffffUL) != 0640UL
        || extended.words[5] != sizeof(attribute_value) - 1) {
        return 51;
    }
#if defined(__aarch64__)
    const long sendfile_number = 71;
    const long preadv_number = 69;
    const long pwritev_number = 70;
#else
    const long sendfile_number = 40;
    const long preadv_number = 295;
    const long pwritev_number = 296;
#endif
    int input = open(base, 0);
    int output = open(copied, 1 | 0x40 | 0x200, 0600);
    long copy_offset = 0;
    if (input < 0 || output < 0
        || syscall(sendfile_number, output, input, &copy_offset,
                   sizeof(attribute_value) - 1) != (long)(sizeof(attribute_value) - 1)
        || copy_offset != (long)(sizeof(attribute_value) - 1) || close(input) != 0
        || close(output) != 0) {
        return 48;
    }
    input = open(copied, 2);
    struct fixture_iovec {
        void *base;
        size_t length;
    } vector;
    char suffix = '!';
    vector.base = &suffix;
    vector.length = 1;
    if (input < 0
        || syscall(pwritev_number, input, &vector, 1L, sizeof(attribute_value) - 1, 0L) != 1) {
        return 49;
    }
    vector.base = buffer;
    vector.length = sizeof(attribute_value);
    if (syscall(preadv_number, input, &vector, 1L, 0L, 0L)
            != (long)sizeof(attribute_value)
        || !same_bytes(buffer, attribute_value, sizeof(attribute_value) - 1)
        || buffer[sizeof(attribute_value) - 1] != '!'
        || close(input) != 0) {
        return 52;
    }
    if (chmod(base, 0600) != 0) {
        return 35;
    }
    if (setxattr(base, attribute, attribute_value, sizeof(attribute_value) - 1, 0) != 0) {
        return 23;
    }
    ssize_t count = getxattr(base, attribute, buffer, sizeof(buffer));
    if (count != (ssize_t)(sizeof(attribute_value) - 1)
        || !same_bytes(buffer, attribute_value, sizeof(attribute_value) - 1)) {
        return 24;
    }
    count = listxattr(base, buffer, sizeof(buffer));
    if (count != (ssize_t)sizeof(attribute)
        || !same_bytes(buffer, attribute, sizeof(attribute))) {
        return 25;
    }
    if (link(base, hard) != 0 || symlink(relative_target, symbolic) != 0) {
        return 26;
    }
    int exclusive = open(base, 1 | 0x40 | 0x80, 0600);
    if (exclusive >= 0) {
        close(exclusive);
        return 38;
    }
    int nofollow = open(symbolic, 0x20000);
    if (nofollow >= 0) {
        close(nofollow);
        return 39;
    }
    count = readlink(symbolic, buffer, sizeof(buffer));
    if (count != (ssize_t)(sizeof(relative_target) - 1)
        || !same_bytes(buffer, relative_target, sizeof(relative_target) - 1)) {
        return 27;
    }
    count = getxattr(hard, attribute, buffer, sizeof(buffer));
    if (count != (ssize_t)(sizeof(attribute_value) - 1)
        || !same_bytes(buffer, attribute_value, sizeof(attribute_value) - 1)) {
        return 28;
    }
    if (removexattr(hard, attribute) != 0
        || getxattr(base, attribute, buffer, sizeof(buffer)) >= 0) {
        return 29;
    }
    if (unlink(symbolic) != 0 || unlink(hard) != 0 || unlink(copied) != 0
        || unlink(base) != 0) {
        return 30;
    }
    return 0;
}

static int memory_abi(void) {
    static const char shared_path[] = "/data/rfc70-shared-map";
    static const char zero_pages[8192];
    char *mapping = (char *)mmap((void *)0, 4096, 1 | 2, 2 | 0x20, -1, 0);
    if (mapping == (void *)-1) {
        return 31;
    }
    mapping[0] = 'm';
    mapping[4095] = 'r';
    char *grown = (char *)mremap(mapping, 4096, 8192, 1);
    if (grown == (void *)-1 || grown[0] != 'm' || grown[4095] != 'r' || grown[4096] != 0) {
        return 32;
    }
    grown[8191] = '!';
    if (munmap(grown, 8192) != 0) {
        return 33;
    }
    unlink(shared_path);
    int fd = open(shared_path, 2 | 0x40 | 0x200, 0600);
    if (fd < 0 || write_all(fd, zero_pages, sizeof(zero_pages)) != 0) {
        return 107;
    }
    char *shared = (char *)mmap((void *)0, 8192, 1 | 2, 1, fd, 0);
    if (shared == (void *)-1) {
        close(fd);
        return 108;
    }
    shared[0] = 'a';
    shared[4096] = 'b';
    if (mprotect(shared + 4096, 4096, 1) != 0 || msync(shared, 8192, 4) != 0
        || munmap(shared, 8192) != 0) {
        close(fd);
        return 109;
    }
    char first = 0;
    char second = 0;
    if (pread(fd, &first, 1, 0) != 1 || pread(fd, &second, 1, 4096) != 1
        || first != 'a' || second != 'b' || close(fd) != 0 || unlink(shared_path) != 0) {
        return 110;
    }
    return 0;
}

static int credential_abi(void) {
    unsigned int requested[2] = {12, 34};
    unsigned int actual[2] = {0, 0};
    if (setgroups(2, requested) != 0 || getgroups(0, (unsigned int *)0) != 2
        || getgroups(2, actual) != 2 || actual[0] != 12 || actual[1] != 34) {
        return 36;
    }
    if (setresgid((unsigned int)-1, (unsigned int)-1, (unsigned int)-1) != 0
        || setresuid((unsigned int)-1, (unsigned int)-1, (unsigned int)-1) != 0) {
        return 37;
    }
    return 0;
}

static int priority_abi(void) {
#if defined(__aarch64__)
    const long setpriority_number = 140;
    const long getpriority_number = 141;
#else
    const long getpriority_number = 140;
    const long setpriority_number = 141;
#endif
    if (syscall(setpriority_number, 0L, 0L, 3L) != 0
        || syscall(getpriority_number, 0L, 0L) != 17) {
        return 45;
    }
    if (syscall(setpriority_number, 0L, 0L, 0L) != 0
        || syscall(getpriority_number, 0L, 0L) != 20) {
        return 46;
    }
    return 0;
}

static int scheduler_abi(void) {
#if defined(__aarch64__)
    const long setparam_number = 118;
    const long setscheduler_number = 119;
    const long getscheduler_number = 120;
    const long getparam_number = 121;
    const long priority_max_number = 125;
    const long priority_min_number = 126;
    const long rr_interval_number = 127;
#else
    const long setparam_number = 142;
    const long getparam_number = 143;
    const long setscheduler_number = 144;
    const long getscheduler_number = 145;
    const long priority_max_number = 146;
    const long priority_min_number = 147;
    const long rr_interval_number = 148;
#endif
    unsigned int priority = 9;
    unsigned long interval[2] = {0, 0};
    if (syscall(getscheduler_number, 0L) != 0
        || syscall(getparam_number, 0L, &priority) != 0 || priority != 0
        || syscall(setparam_number, 0L, &priority) != 0
        || syscall(setscheduler_number, 0L, 0L, &priority) != 0
        || syscall(priority_max_number, 0L) != 0 || syscall(priority_min_number, 0L) != 0
        || syscall(rr_interval_number, 0L, interval) != 0
        || (interval[0] == 0 && interval[1] == 0)) {
        return 47;
    }
    return 0;
}

static int pending_signal_abi(void) {
    unsigned long blocked = 1UL << 9;
    unsigned long pending = 0;
    if (sigprocmask(0, &blocked, (void *)0) != 0 || kill(getpid(), 10) != 0
        || sigpending(&pending) != 0 || (pending & blocked) == 0) {
        return 53;
    }
    return 0;
}

static volatile int delivered_signal;

static void record_signal(int signal_number) {
    delivered_signal = signal_number;
}

static int signal_wait_abi(void) {
    unsigned long empty = 0;
    int parent = getpid();
    if (signal(12, record_signal) == (void *)-1) {
        return 54;
    }
    int child = fork();
    if (child < 0) {
        return 55;
    }
    if (child == 0) {
        int result = kill(parent, 12);
        _exit(result == 0 ? 0 : 56);
    }
    if (sigsuspend(&empty) != -1 || delivered_signal != 12) {
        return 57;
    }
    int status = 0;
    if (waitpid(child, &status, 0) != child || status != 0) {
        return 58;
    }
    return 0;
}

struct fixture_robust_node {
    unsigned long next;
    unsigned int futex;
};

struct fixture_robust_head {
    unsigned long next;
    long futex_offset;
    unsigned long pending;
};

static struct fixture_robust_node robust_node;
static struct fixture_robust_head robust_head;

static int robust_child(void *argument) {
    (void)argument;
#if defined(__aarch64__)
    const long gettid_number = 178;
    const long set_robust_number = 99;
    const long get_robust_number = 100;
#else
    const long gettid_number = 186;
    const long set_robust_number = 273;
    const long get_robust_number = 274;
#endif
    unsigned long observed_head = 0;
    unsigned long observed_length = 0;
    robust_head.next = (unsigned long)&robust_node;
    robust_head.futex_offset = (char *)&robust_node.futex - (char *)&robust_node;
    robust_head.pending = 0;
    robust_node.next = (unsigned long)&robust_head;
    robust_node.futex = (unsigned int)syscall(gettid_number) | 0x80000000U;
    if (syscall(set_robust_number, &robust_head, 24UL) != 0
        || syscall(get_robust_number, 0L, &observed_head, &observed_length) != 0
        || observed_head != (unsigned long)&robust_head || observed_length != 24) {
        return 59;
    }
    return 0;
}

static int robust_futex_abi(void) {
    char *stack = (char *)mmap((void *)0, 65536, 1 | 2, 2 | 0x20, -1, 0);
    if (stack == (void *)-1) {
        return 60;
    }
    int child = clone(robust_child, stack + 65536, 0x100 | 17, (void *)0);
    if (child < 0) {
        munmap(stack, 65536);
        return 61;
    }
    int status = 0;
    if (waitpid(child, &status, 0) != child || status != 0
        || robust_node.futex != 0xc0000000U || munmap(stack, 65536) != 0) {
        return 62;
    }
    return 0;
}

static volatile unsigned int bitset_futex;
static volatile unsigned int bitset_ready;

static int bitset_child(void *argument) {
    (void)argument;
#if defined(__aarch64__)
    const long futex_number = 98;
#else
    const long futex_number = 202;
#endif
    bitset_ready = 1;
    if (syscall(futex_number, &bitset_ready, 1L, 1L, (void *)0, (void *)0, 0L) != 1) {
        return 40;
    }
    return syscall(futex_number, &bitset_futex, 9L, 0L, (void *)0, (void *)0, 2U) == 0
        ? 0 : 41;
}

static int futex_bitset_abi(void) {
#if defined(__aarch64__)
    const long futex_number = 98;
#else
    const long futex_number = 202;
#endif
    bitset_futex = 0;
    bitset_ready = 0;
    if (syscall(futex_number, &bitset_futex, 9L, 0L, (void *)0, (void *)0, 0U) >= 0
        || syscall(futex_number, &bitset_futex, 10L, 1L, (void *)0, (void *)0, 0U) >= 0) {
        return 42;
    }
    char *stack = (char *)mmap((void *)0, 65536, 1 | 2, 2 | 0x20, -1, 0);
    if (stack == (void *)-1) {
        return 43;
    }
    int child = clone(bitset_child, stack + 65536, 0x100 | 17, (void *)0);
    if (child < 0) {
        munmap(stack, 65536);
        return 44;
    }
    if (syscall(futex_number, &bitset_ready, 0L, 0L, (void *)0, (void *)0, 0L) != 0
        || syscall(futex_number, &bitset_futex, 10L, 1L, (void *)0, (void *)0, 1U) != 0
        || syscall(futex_number, &bitset_futex, 10L, 1L, (void *)0, (void *)0, 2U) != 1) {
        return 45;
    }
    int status = 0;
    if (waitpid(child, &status, 0) != child || status != 0 || munmap(stack, 65536) != 0) {
        return 46;
    }
    if (syscall(futex_number, &bitset_futex, 9 | 128, 1L, (void *)0, (void *)0, ~0u) >= 0) {
        return 47;
    }
    if (syscall(futex_number, &bitset_futex, 10 | 128, 1L, (void *)0, (void *)0, ~0u)
        != 0) {
        return 41;
    }
    return 0;
}

static volatile unsigned int pi_futex_word;
static volatile unsigned int pi_child_ready;

static int pi_futex_child(void *argument) {
    (void)argument;
#if defined(__aarch64__)
    const long futex_number = 98;
    const long gettid_number = 178;
#else
    const long futex_number = 202;
    const long gettid_number = 186;
#endif
    pi_child_ready = 1;
    if (syscall(futex_number, &pi_child_ready, 1L, 1L, (void *)0, (void *)0, 0L) != 1) {
        return 63;
    }
    if (syscall(futex_number, &pi_futex_word, 6L, 0L, (void *)0, (void *)0, 0L) != 0) {
        return 64;
    }
    unsigned int tid = (unsigned int)syscall(gettid_number);
    if ((pi_futex_word & 0x3fffffffU) != tid
        || syscall(futex_number, &pi_futex_word, 7L, 0L, (void *)0, (void *)0, 0L) != 0) {
        return 65;
    }
    return 0;
}

static int pi_futex_abi(void) {
#if defined(__aarch64__)
    const long futex_number = 98;
#else
    const long futex_number = 202;
#endif
    pi_futex_word = 0;
    pi_child_ready = 0;
    if (syscall(futex_number, &pi_futex_word, 6L, 0L, (void *)0, (void *)0, 0L) != 0
        || syscall(futex_number, &pi_futex_word, 8L, 0L, (void *)0, (void *)0, 0L) >= 0) {
        return 66;
    }
    char *stack = (char *)mmap((void *)0, 65536, 1 | 2, 2 | 0x20, -1, 0);
    if (stack == (void *)-1) {
        return 67;
    }
    int child = clone(pi_futex_child, stack + 65536, 0x100 | 17, (void *)0);
    if (child < 0) {
        munmap(stack, 65536);
        return 68;
    }
    if (syscall(futex_number, &pi_child_ready, 0L, 0L, (void *)0, (void *)0, 0L) != 0
        || pi_child_ready != 1 || (pi_futex_word & 0x80000000U) == 0
        || syscall(futex_number, &pi_futex_word, 7L, 0L, (void *)0, (void *)0, 0L) != 0) {
        return 69;
    }
    int status = 0;
    if (waitpid(child, &status, 0) != child || status != 0 || pi_futex_word != 0
        || syscall(futex_number, &pi_futex_word, 7L, 0L, (void *)0, (void *)0, 0L) >= 0
        || munmap(stack, 65536) != 0) {
        return 70;
    }
    return 0;
}

struct fixture_futex_waitv {
    unsigned long value;
    unsigned long address;
    unsigned int flags;
    unsigned int reserved;
};

static volatile unsigned int waitv_first;
static volatile unsigned int waitv_second;

static int waitv_child(void *argument) {
    (void)argument;
#if defined(__aarch64__)
    const long futex_number = 98;
#else
    const long futex_number = 202;
#endif
    waitv_second = 1;
    return syscall(futex_number, &waitv_second, 1L, 1L, (void *)0, (void *)0, 0L) == 1
        ? 0 : 71;
}

static int futex_waitv_abi(void) {
    const long futex_waitv_number = 449;
    waitv_first = 0;
    waitv_second = 0;
    struct fixture_futex_waitv waiters[2] = {
        {0, (unsigned long)&waitv_first, 2, 0},
        {0, (unsigned long)&waitv_second, 2, 0},
    };
    char *stack = (char *)mmap((void *)0, 65536, 1 | 2, 2 | 0x20, -1, 0);
    if (stack == (void *)-1) {
        return 72;
    }
    int child = clone(waitv_child, stack + 65536, 0x100 | 17, (void *)0);
    if (child < 0) {
        munmap(stack, 65536);
        return 73;
    }
    if (syscall(futex_waitv_number, waiters, 2L, 0L, (void *)0, 1L) != 1) {
        return 74;
    }
    int status = 0;
    if (waitv_second != 1 || waitpid(child, &status, 0) != child || status != 0
        || munmap(stack, 65536) != 0) {
        return 75;
    }
    return 0;
}

static volatile unsigned int requeue_source;
static volatile unsigned int requeue_target;
static volatile unsigned int requeue_ready;

static int requeue_child(void *argument) {
    (void)argument;
#if defined(__aarch64__)
    const long futex_number = 98;
#else
    const long futex_number = 202;
#endif
    requeue_ready = 1;
    if (syscall(futex_number, &requeue_ready, 1L, 1L, (void *)0, (void *)0, 0L) != 1) {
        return 76;
    }
    return syscall(futex_number, &requeue_source, 0L, 0L, (void *)0, (void *)0, 0L) == 0
        ? 0 : 77;
}

static int futex_requeue_abi(void) {
#if defined(__aarch64__)
    const long futex_number = 98;
#else
    const long futex_number = 202;
#endif
    requeue_source = 0;
    requeue_target = 0;
    requeue_ready = 0;
    char *stack = (char *)mmap((void *)0, 65536, 1 | 2, 2 | 0x20, -1, 0);
    if (stack == (void *)-1) {
        return 78;
    }
    int child = clone(requeue_child, stack + 65536, 0x100 | 17, (void *)0);
    if (child < 0) {
        munmap(stack, 65536);
        return 79;
    }
    if (syscall(futex_number, &requeue_ready, 0L, 0L, (void *)0, (void *)0, 0L) != 0
        || syscall(futex_number, &requeue_source, 3L, 0L, 1L, &requeue_target, 0L) != 1
        || syscall(futex_number, &requeue_target, 1L, 1L, (void *)0, (void *)0, 0L) != 1) {
        return 80;
    }
    int status = 0;
    if (waitpid(child, &status, 0) != child || status != 0 || munmap(stack, 65536) != 0) {
        return 81;
    }
    return 0;
}

static volatile unsigned int wake_op_source;
static volatile unsigned int wake_op_target;
static volatile unsigned int wake_op_ready;

static int wake_op_child(void *argument) {
    (void)argument;
#if defined(__aarch64__)
    const long futex_number = 98;
#else
    const long futex_number = 202;
#endif
    wake_op_ready = 1;
    if (syscall(futex_number, &wake_op_ready, 1L, 1L, (void *)0, (void *)0, 0L) != 1) {
        return 82;
    }
    return syscall(futex_number, &wake_op_target, 0L, 0L, (void *)0, (void *)0, 0L) == 0
        ? 0 : 83;
}

static int futex_wake_op_abi(void) {
#if defined(__aarch64__)
    const long futex_number = 98;
#else
    const long futex_number = 202;
#endif
    const unsigned int add_one_if_old_zero = (1U << 28) | (1U << 12);
    wake_op_source = 0;
    wake_op_target = 0;
    wake_op_ready = 0;
    char *stack = (char *)mmap((void *)0, 65536, 1 | 2, 2 | 0x20, -1, 0);
    if (stack == (void *)-1) {
        return 84;
    }
    int child = clone(wake_op_child, stack + 65536, 0x100 | 17, (void *)0);
    if (child < 0) {
        munmap(stack, 65536);
        return 85;
    }
    if (syscall(futex_number, &wake_op_ready, 0L, 0L, (void *)0, (void *)0, 0L) != 0
        || syscall(futex_number, &wake_op_source, 5L, 0L, 1L, &wake_op_target,
               add_one_if_old_zero) != 1
        || wake_op_target != 1) {
        return 86;
    }
    int status = 0;
    if (waitpid(child, &status, 0) != child || status != 0 || munmap(stack, 65536) != 0) {
        return 87;
    }
    return 0;
}

static volatile unsigned int pi_requeue_source;
static volatile unsigned int pi_requeue_target;
static volatile unsigned int pi_requeue_ready;

static int pi_requeue_child(void *argument) {
    (void)argument;
#if defined(__aarch64__)
    const long futex_number = 98;
    const long gettid_number = 178;
#else
    const long futex_number = 202;
    const long gettid_number = 186;
#endif
    pi_requeue_ready = 1;
    if (syscall(futex_number, &pi_requeue_ready, 1L, 1L, (void *)0, (void *)0, 0L) != 1) {
        return 88;
    }
    if (syscall(futex_number, &pi_requeue_source, 11L, 0L, (void *)0,
            &pi_requeue_target, 0L) != 0) {
        return 89;
    }
    unsigned int tid = (unsigned int)syscall(gettid_number);
    if ((pi_requeue_target & 0x3fffffffU) != tid
        || syscall(futex_number, &pi_requeue_target, 7L, 0L, (void *)0, (void *)0, 0L)
            != 0) {
        return 90;
    }
    return 0;
}

static int futex_pi_requeue_abi(void) {
#if defined(__aarch64__)
    const long futex_number = 98;
#else
    const long futex_number = 202;
#endif
    pi_requeue_source = 0;
    pi_requeue_target = 0;
    pi_requeue_ready = 0;
    if (syscall(futex_number, &pi_requeue_target, 6L, 0L, (void *)0, (void *)0, 0L) != 0) {
        return 91;
    }
    char *stack = (char *)mmap((void *)0, 65536, 1 | 2, 2 | 0x20, -1, 0);
    if (stack == (void *)-1) {
        return 92;
    }
    int child = clone(pi_requeue_child, stack + 65536, 0x100 | 17, (void *)0);
    if (child < 0) {
        munmap(stack, 65536);
        return 93;
    }
    if (syscall(futex_number, &pi_requeue_ready, 0L, 0L, (void *)0, (void *)0, 0L) != 0
        || syscall(futex_number, &pi_requeue_source, 12L, 1L, 0L, &pi_requeue_target, 0L)
            != 1
        || (pi_requeue_target & 0x80000000U) == 0
        || syscall(futex_number, &pi_requeue_target, 7L, 0L, (void *)0, (void *)0, 0L)
            != 0) {
        return 94;
    }
    int status = 0;
    if (waitpid(child, &status, 0) != child || status != 0 || pi_requeue_target != 0
        || munmap(stack, 65536) != 0) {
        return 95;
    }
    return 0;
}

struct fixture_rseq {
    unsigned int cpu_id_start;
    unsigned int cpu_id;
    unsigned long rseq_cs;
    unsigned int flags;
    unsigned char padding[12];
} __attribute__((aligned(32)));

static struct fixture_rseq rseq_area;

static int rseq_child(void *argument) {
    (void)argument;
#if defined(__aarch64__)
    const long rseq_number = 293;
#else
    const long rseq_number = 334;
#endif
    const unsigned int signature = 0x53053053U;
    rseq_area.cpu_id_start = ~0U;
    rseq_area.cpu_id = ~0U;
    if (sizeof(rseq_area) != 32
        || syscall(rseq_number, &rseq_area, sizeof(rseq_area), 0L, signature) != 0
        || rseq_area.cpu_id_start != 0 || rseq_area.cpu_id != 0) {
        return 96;
    }
    if (syscall(rseq_number, &rseq_area, sizeof(rseq_area), 0L, signature) >= 0) {
        return 97;
    }
    if (syscall(rseq_number, &rseq_area, sizeof(rseq_area), 1L, signature ^ 1U) >= 0
        || syscall(rseq_number, &rseq_area, sizeof(rseq_area), 1L, signature) != 0
        || rseq_area.cpu_id_start != 0 || rseq_area.cpu_id != ~0U) {
        return 98;
    }
    if (syscall(rseq_number, &rseq_area, sizeof(rseq_area) - 1, 0L, signature) >= 0) {
        return 99;
    }
    return 0;
}

static int rseq_abi(void) {
    char *stack = (char *)mmap((void *)0, 65536, 1 | 2, 2 | 0x20, -1, 0);
    if (stack == (void *)-1) {
        return 100;
    }
    int child = clone(rseq_child, stack + 65536, 0x100 | 17, (void *)0);
    if (child < 0) {
        munmap(stack, 65536);
        return 101;
    }
    int status = 0;
    if (waitpid(child, &status, 0) != child || status != 0 || munmap(stack, 65536) != 0) {
        return 102;
    }
    return 0;
}

int main(void) {
    static const char child_message[] = "child local ipc\n";
    static const char success[] = "dynamic musl process ok\n";
    char received[sizeof(child_message) - 1];
    int fds[2];
    int status = 0;
    int proc_maps_status = proc_maps_abi();
    if (proc_maps_status != 0) {
        return proc_maps_status;
    }
    int filesystem_status = filesystem_abi();
    if (filesystem_status != 0) {
        return filesystem_status;
    }
    int memory_status = memory_abi();
    if (memory_status != 0) {
        return memory_status;
    }
    int credential_status = credential_abi();
    if (credential_status != 0) {
        return credential_status;
    }
    int priority_status = priority_abi();
    if (priority_status != 0) {
        return priority_status;
    }
    int scheduler_status = scheduler_abi();
    if (scheduler_status != 0) {
        return scheduler_status;
    }
    int pending_signal_status = pending_signal_abi();
    if (pending_signal_status != 0) {
        return pending_signal_status;
    }
    int signal_wait_status = signal_wait_abi();
    if (signal_wait_status != 0) {
        return signal_wait_status;
    }
    int robust_futex_status = robust_futex_abi();
    if (robust_futex_status != 0) {
        return robust_futex_status;
    }
    int futex_status = futex_bitset_abi();
    if (futex_status != 0) {
        return futex_status;
    }
    int pi_futex_status = pi_futex_abi();
    if (pi_futex_status != 0) {
        return pi_futex_status;
    }
    int futex_waitv_status = futex_waitv_abi();
    if (futex_waitv_status != 0) {
        return futex_waitv_status;
    }
    int futex_requeue_status = futex_requeue_abi();
    if (futex_requeue_status != 0) {
        return futex_requeue_status;
    }
    int futex_wake_op_status = futex_wake_op_abi();
    if (futex_wake_op_status != 0) {
        return futex_wake_op_status;
    }
    int futex_pi_requeue_status = futex_pi_requeue_abi();
    if (futex_pi_requeue_status != 0) {
        return futex_pi_requeue_status;
    }
    int rseq_status = rseq_abi();
    if (rseq_status != 0) {
        return rseq_status;
    }
    if (pipe(fds) != 0) {
        return 10;
    }
    int child = fork();
    if (child < 0) {
        return 11;
    }
    if (child == 0) {
        close(fds[0]);
        int result = write_all(fds[1], child_message, sizeof(child_message) - 1);
        close(fds[1]);
        _exit(result == 0 ? 7 : 12);
    }
    close(fds[1]);
    size_t offset = 0;
    while (offset != sizeof(received)) {
        ssize_t count = read(fds[0], received + offset, sizeof(received) - offset);
        if (count <= 0) {
            return 13;
        }
        offset += (size_t)count;
    }
    close(fds[0]);
    if (waitpid(child, &status, 0) != child || status != (7 << 8)) {
        return 14;
    }
    for (size_t index = 0; index != sizeof(received); ++index) {
        if (received[index] != child_message[index]) {
            return 15;
        }
    }
    return write_all(1, success, sizeof(success) - 1) == 0 ? 0 : 16;
}
