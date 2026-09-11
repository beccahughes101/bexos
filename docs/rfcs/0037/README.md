# RFC 0037: Graphical boot and display handoff

- Created: 2026-08-30T15:03:41-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

Boot services, splashd, and the compositor transfer display ownership while preserving continuous graphical feedback. The long-term handoff design is retained alongside the current boot UI milestone.

## Design overview

> Implementation status: the graphical product now has a CPU boot UI and a minimal
> compositor implementation. See [current boot UI](../../bootui.md) for its
> supported paths, validation, and limits. The broader design below remains the
> long-term architecture.

Graphical boot uses BexOS's capability and process lifecycle model to transfer display ownership without interrupting animation.

The primary challenge in a microkernel is maintaining smooth 60fps animations while the system transitions from the initial firmware framebuffer to userspace drivers, mounts encrypted storage, and launches the full UI compositor without screen flashes or black frames.

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ 1. BOOTLOADER (UEFI GOP / Device Tree SimpleFB)                             │
│    • Allocates physical framebuffer (e.g. 1920x1080x32bpp)                  │
│    • Draws static OEM/BexOS logo frame                                      │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ Framebuffer Base + Dimensions Handover
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ 2. D0 MICROKERNEL (Early Boot)                                              │
│    • Wraps physical framebuffer memory as a restricted physical `VMO`       │
│    • Passes `framebuffer_vmo` capability directly to `bootsvc` / `splashd`  │
│    • Microkernel contains ZERO graphics drawing code                        │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ Injects `framebuffer_vmo` handle
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ 3. `splashd` (Early Boot Splash Daemon - D1 Userspace)                      │
│    • Lightweight, statically linked self-contained binary (<500KB)         │
│    • Double-buffered software rasterizer (plays spinner/vector animation)   │
│    • Listens to system progress milestones over FIDL control channel        │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ 4. Handover display authority
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ 4. FULL UI COMPOSITOR (`scened` / GUI Session)                              │
│    • Full GPU driver (VirtIO-GPU / DRM / Vulkan) initializes in userspace   │
│    • `scened` takes over display scanout with seamless cross-fade           │
│    • `splashd` terminates and frees its memory                              │
└─────────────────────────────────────────────────────────────────────────────┘

```

## Phase 1: Zero-Kernel Drawing & the `bootsvc` Handover

To keep the microkernel small and secure:

1. The **UEFI bootloader (GOP)** initializes the physical display output and leaves the initial splash logo on the screen.
2. The bootloader passes the framebuffer physical memory descriptor to **D0**.
3. **D0** creates a capability handle: a physical Memory Object (`handle:VMO`) pointing to that scanout region, and delegates it to the root userspace bootstrap process (`bootsvc`).
4. `bootsvc` immediately spawns **`splashd`** before mounting disks or initializing `appd`/`netstack`.

## Phase 2: `splashd` Animation Loop & Progress Channel

`splashd` is a minimalist, standalone service that runs without depending on dynamic libraries, VFS, or complex GPU stacks:

* **Rendering:** Uses a compact in-process rasterizer (like `tiny-skia` or flat frame sequences) to render smooth spinner loops or pulsing brand animations into a back-buffer VMO, then copies dirty rectangles to the mapped hardware scanout VMO.
* **Progress Tracking:** Exposes a simple FIDL control channel (`bexos.splash.Progress`) that other early services ping as they initialize.

```fidl
library bexos.splash;

using bexos.kernel;

type BootStage = strict enum : uint8 {
    KERNEL_BOOTSTRAP     = 1;
    STORAGE_UNLOCKED     = 2; // RoseFS / Vault ready
    DRIVERS_INITIALIZED  = 3; // Netstack / Display drivers online
    SYSTEM_APPS_ONLINE   = 4; // appd ready
    READY_FOR_COMPOSITOR = 5; // scened taking over
};

@discoverable
protocol ProgressTracker {
    /// Update boot progress state to drive animation stage or progress bar
    ReportStage(struct {
        stage BootStage;
        progress_pct uint8; // 0 - 100
        status_message string:64;
    }) -> (struct {
        status bexos.kernel.Status;
    });

    /// Instruct splashd to freeze/fade and release the display VMO
    HandoverToCompositor(resource struct {
        ack_channel handle:CHANNEL;
    }) -> (struct {
        status bexos.kernel.Status;
    });
};

```

## Phase 3: Seamless GPU & Compositor Handover

The critical step in eliminating screen flicker (the classic "black flash" seen during OS boot):

1. `scened` (the main BexOS UI compositor) starts and initializes the real GPU driver (`d1_virtio_gpu` or native DRM/Vulkan driver).
2. `scened` calls `splashd.HandoverToCompositor()`.
3. `splashd` renders its final frame and stops writing to the dumb framebuffer.
4. `scened` copies the current screen contents into its initial GPU swapchain texture and begins scanning out from the GPU at the identical pixel positions.
5. `scened` sends the acknowledgement signal back over `ack_channel`.
6. `splashd` unmaps the framebuffer VMO and exits, freeing its memory back to the system.

## Benefits of This Architecture

* **Instant Visual Feedback:** Animation starts within milliseconds of the microkernel entering userland, long before heavy storage encryption, network stacks, or application runtimes finish loading.
* **Fault Resilient:** If `splashd` encounters an unexpected error or panics, it cannot crash the microkernel or halt boot—the system continues booting silently into the compositor.
* **Encrypted Vault Integration:** While `splashd` is animating on screen, `vfsd` and `teed` can prompt the user for home vault passcodes or biometric unlock in a high-priority, secure overlay.
