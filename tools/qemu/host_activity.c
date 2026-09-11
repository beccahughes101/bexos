#import <Foundation/Foundation.h>
#include <stdio.h>
#include <stdlib.h>

// Loaded only into the QEMU child. Retaining this token keeps its device I/O
// and timers responsive when Cocoa is occluded, without preventing system sleep.
static id activity;

__attribute__((constructor)) static void begin_qemu_activity(void) {
    @autoreleasepool {
        activity = [[[NSProcessInfo processInfo]
            beginActivityWithOptions:NSActivityUserInitiatedAllowingIdleSystemSleep
            reason:@"Running the BexOS virtual workstation"] retain];
        if (!activity) {
            fputs("qemu: cannot request macOS background activity\n", stderr);
            abort();
        }
        fputs("qemu: macOS background activity enabled\n", stderr);
    }
}
