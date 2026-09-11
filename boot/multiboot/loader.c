/* Freestanding loader. The packer validates ELF layout before embedding it. */
#include "payload.h"
typedef unsigned int u32;
typedef unsigned long long u64;
extern const unsigned char payload_start[];
extern void enter_trusty(u32) __attribute__((noreturn));
extern void enter_multiboot2(u32, u32) __attribute__((noreturn));
u64 pml4[512] __attribute__((aligned(4096)));
static u64 pdpt[512] __attribute__((aligned(4096)));
static u64 pd[2048] __attribute__((aligned(4096)));
static u32 tags[8192] __attribute__((aligned(8)));
static unsigned used;
static void copy(void *to, const void *from, unsigned size) {
    unsigned char *d = to;
    const unsigned char *s = from;
    while (size--) *d++ = *s++;
}
static void fail(void) {
    const char *p = "bexos-loader: invalid boot handoff\n";
    while (*p) { __asm__ volatile("outb %0, %1" :: "a"(*p++), "Nd"((unsigned short)0x3f8)); }
    for (;;) __asm__ volatile("cli; hlt");
}
static void *tag(unsigned type, unsigned size) {
    if (size < 8 || used + ((size + 7) & ~7u) > sizeof(tags)) fail();
    u32 *p = (u32 *)((char *)tags + used);
    used += (size + 7) & ~7u;
    p[0] = type; p[1] = size;
    return p;
}
static u32 multiboot2_info(u32 magic, const u32 *info) {
    if (magic == 0x36d76289) {
        if (((u32)info & 7) || info[0] < 16 || info[0] > sizeof(tags) || (info[0] & 7) || info[1]) fail();
        unsigned cursor = 8;
        int ended = 0;
        while (cursor <= info[0] - 8) {
            const u32 *item = (const void *)((const char *)info + cursor);
            if (item[1] < 8 || item[1] > info[0] - cursor) fail();
            if (!item[0]) {
                if (item[1] != 8 || cursor + 8 != info[0]) fail();
                ended = 1;
                break;
            }
            cursor += (item[1] + 7) & ~7u;
        }
        if (!ended) fail();
        copy(tags, info, info[0]);
        return (u32)tags;
    }
    if (magic != 0x2badb002 || !(info[0] & 1)) fail();
    used = 8;
    u32 *mem = tag(4, 16); mem[2] = info[1]; mem[3] = info[2];
    if (info[0] & (1 << 6)) {
        const unsigned char *p = (const void *)info[12];
        unsigned left = info[11];
        unsigned start = used;
        u32 *map = tag(6, 16); map[2] = 24; map[3] = 0;
        while (left) {
            if (left < 24) fail();
            u32 size = *(const u32 *)p;
            if (size < 20 || size > left-4 || used + 24 > sizeof(tags)-8) fail();
            copy((char *)tags+used, p+4, 20); used += 24;
            p += size+4; left -= size+4;
        }
        map[1] = used-start;
    }
    if (info[0] & (1 << 3)) {
        if (info[5] > 32) fail();
        const u32 *module = (const void *)info[6];
        for (unsigned i=0; i<info[5]; ++i, module+=4) {
            const char *name = (const void *)module[2];
            unsigned length = 0;
            while (name && length < 255 && name[length]) ++length;
            u32 *m = tag(3, 17+length);
            if (module[1] < module[0]) fail();
            m[2] = module[0]; m[3] = module[1];
            if (length) copy(m+4, name, length);
        }
    }
    if (info[0] & (1u << 12)) {
        const unsigned char *raw = (const void *)info;
        unsigned char *fb = tag(8, 38);
        copy(fb + 8, raw + 88, 22);
        copy(fb + 32, raw + 110, 6);
    }
    tag(0, 8); tags[0] = used;
    return (u32)tags;
}
void load_payload(u32 magic, const u32 *info) {
    u32 mb2 = multiboot2_info(magic, info);
    for (unsigned i=0; i<PAYLOAD_SEGMENTS; ++i) {
        const struct segment *s = &segments[i];
        copy((void *)s->address, payload_start+s->offset, s->filesz);
        unsigned char *bss = (void *)(s->address+s->filesz);
        for (unsigned j=s->filesz; j<s->memsz; ++j) *bss++ = 0;
    }
    if (!PAYLOAD_TRUSTY) enter_multiboot2(PAYLOAD_ENTRY, mb2);
    pml4[0] = (u32)pdpt | 3;
    for (unsigned i=0; i<4; ++i) pdpt[i] = (u32)&pd[i*512] | 3;
    for (unsigned i=0; i<2048; ++i) pd[i] = ((u64)i << 21) | 0x83;
    enter_trusty(PAYLOAD_ENTRY);
}
