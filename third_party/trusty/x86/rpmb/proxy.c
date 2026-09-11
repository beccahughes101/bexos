/* Standalone QEMU development transport. Authenticated RPMB frames are opaque.
 * A reboot closes the IPC session; the host-owned RPMB image survives it.
 * This module is not linked into BexOS or any production Trusty profile. */
#include <err.h>
#include <debug.h>
#include <kernel/thread.h>
#include <lib/ktipc/ktipc.h>
#include <lib/trusty/ipc.h>
#include <lib/trusty/uuid.h>
#include <lk/init.h>
#include <string.h>
#include <stdatomic.h>

#define UART 0x2f8
#define MAX_FRAME_BYTES 4096
static unsigned char request[8192];
static unsigned char response[24 + MAX_FRAME_BYTES];
/* One-way boot ownership transfer. Stop only between complete authenticated
 * frame exchanges, close storage IPC, then acknowledge the new owner. */
static atomic_uint phase;
enum { STARTING, ACTIVE, RELEASING, RELEASED };
int bexos_boot_rpmb_release(void) {
    unsigned expected = ACTIVE;
    if (!atomic_compare_exchange_strong(&phase, &expected, RELEASING))
        return expected == RELEASED ? NO_ERROR : ERR_BAD_STATE;
    for (unsigned waits = 0; waits < 5000; ++waits) {
        if (atomic_load(&phase) == RELEASED) return NO_ERROR;
        thread_sleep(1);
    }
    return ERR_TIMED_OUT;
}
static unsigned char inb(unsigned short port) {
    unsigned char byte;
    __asm__ volatile("inb %1, %0" : "=a"(byte) : "Nd"(port));
    return byte;
}
static void outb(unsigned short port, unsigned char byte) {
    __asm__ volatile("outb %0, %1" :: "a"(byte), "Nd"(port));
}
static int transfer(unsigned char *bytes, size_t size, bool read) {
    /* One bounded exchange; a disconnected host cannot stall Trusty forever. */
    unsigned waits = 0;
    for (size_t i=0; i<size; ++i) {
        while (!(inb(UART+5) & (read ? 1 : 0x20))) {
            if (++waits > 60000) return ERR_TIMED_OUT;
            thread_sleep(1);
        }
        if (read) bytes[i] = inb(UART); else outb(UART, bytes[i]);
    }
    return NO_ERROR;
}
static uint32_t word(const unsigned char *p) { uint32_t v; memcpy(&v,p,4); return v; }
static void put(unsigned char *p, uint32_t v) { memcpy(p,&v,4); }
static int dispatch(size_t size) {
    if (size < 24 || word(request+12) != size || (word(request)&1) || word(request+16) || word(request+20)) return ERR_BAD_LEN;
    memcpy(response,request,24);
    put(response,word(request)|1); put(response+8,0);
    static bool reported_exchange;
    uint32_t result = 0;
    size_t length = 24;
    uint32_t command = word(request);
    if (word(request+8)&~0x1eu) result = 2;
    else if (command == 16) {
        if (size < 40) result = 2;
        else {
            uint32_t reliable=word(request+24), written=word(request+28), read=word(request+32);
            uint64_t total=(uint64_t)reliable+written;
            if (!total || total > MAX_FRAME_BYTES || total != size-40 || reliable%512 || written%512 || !read || read > MAX_FRAME_BYTES || read%512 || word(request+36)) result=2;
            else {
                uint16_t counts[2]={(uint16_t)(read/512),(uint16_t)(total/512)};
                if (transfer((void *)counts,sizeof(counts),false) || transfer(request+40,total,false) || transfer(response+24,read,true)) result=1;
                else {
                    length+=read;
                    if (!reported_exchange) {
                        dprintf(ALWAYS,"bexos-trusty-x86: RPMB exchange complete\n");
                        reported_exchange = true;
                    }
                }
            }
        }
    } else result = command == 4 ? 5 : 3;
    put(response+12,length); put(response+16,result);
    return length;
}
static int proxy(void *unused) {
    (void)unused;
    outb(UART+1,0); outb(UART+3,0x80); outb(UART,1); outb(UART+1,0);
    outb(UART+3,3); outb(UART+2,7); outb(UART+4,3);
    struct handle *channel = NULL;
    int rc=ipc_port_connect_async(&kernel_uuid,"com.android.trusty.storage.proxy",IPC_PORT_PATH_MAX,IPC_CONNECT_WAIT_FOR_PORT,&channel);
    if (rc) return rc;
    atomic_store(&phase, ACTIVE);
    uint32_t event;
    for (;;) {
        if (atomic_load(&phase) == RELEASING) { rc = NO_ERROR; break; }
        rc=handle_wait(channel,&event,50);
        if (rc == ERR_TIMED_OUT) continue;
        if (rc || (event&IPC_HANDLE_POLL_HUP)) break;
        if (!(event&IPC_HANDLE_POLL_MSG)) continue;
        rc=ktipc_recv(channel,24,request,sizeof(request));
        if (rc < 0) break;
        rc=dispatch(rc);
        if (rc < 0) break;
        rc=ktipc_send(channel,response,(size_t)rc);
        if (rc < 0) break;
    }
    handle_close(channel);
    if (rc == NO_ERROR && atomic_load(&phase) == RELEASING) {
        atomic_store(&phase, RELEASED);
        dprintf(ALWAYS,"bexos-trusty-x86: boot RPMB owner released\n");
        return NO_ERROR;
    }
    dprintf(CRITICAL,"bexos-trusty-x86: RPMB transport stopped (%d)\n",rc);
    return rc;
}
static void initialize(uint level) {
    (void)level;
    thread_t *thread=thread_create("bexos-qemu-rpmb",proxy,NULL,DEFAULT_PRIORITY,16384);
    if (!thread) panic("create standalone RPMB proxy thread");
    thread_resume(thread);
    dprintf(ALWAYS,"bexos-trusty-x86: ready\n");
}
LK_INIT_HOOK(bexos_qemu_rpmb, initialize, LK_INIT_LEVEL_APPS);
