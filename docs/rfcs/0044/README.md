# RFC 0044: Graphics drivers and kernel buffer allocation

- Created: 2026-09-01T10:47:26-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

The kernel allocates physical backing through VMOs, while D1 graphics drivers own device behavior. Shared buffers connect rendering, display swapchains, and VSYNC presentation.

## Design overview

The D0 microkernel owns physical buffer allocation through capability-scoped memory objects.

Having the kernel manage physical frame allocation directly simplifies IPC, improves latency, and eliminates circular dependency deadlocks between userspace drivers and an external memory daemon.

## Why In-Kernel Allocation Fits Better Than a Userspace `allocatord`

* **Hardware Authority:** The microkernel already controls page tables, IOMMU contexts, and physical page frames (buddy allocator). Having an external userspace daemon manage physical memory introduces redundant syscall round-trips (`userspace -> allocatord -> kernel -> allocatord -> userspace`).
* **Zero-Copy Capability Grants:** When the kernel creates a `VMO`, it can directly populate contiguous physical frames or IOMMU-pinned scatter-gather lists and mint a capability handle in a single step.
* **Elimination of IPC Deadlocks:** If a userspace memory allocator crashes or exhausts its channel queue during a display swapchain allocation or DMA transfer, the entire graphics pipeline halts. In-kernel allocation ensures guaranteed, non-blocking page-frame provisioning.

## In-Kernel VMO Architecture

In-kernel memory allocation is driven entirely via the universal `svc #1` FIDL multiplexer under the `bexos.kernel.Vmo` protocol (`protocol = 0x01`):

```fidl
library bexos.kernel;

type VmoFlags = strict bits : uint32 {
    CONTIGUOUS_PHYSICAL = 0b00000001; // Required for hardware scanout/CRTC
    CACHE_POLICY_UNCACHED = 0b00000010;
    CACHE_POLICY_WRITE_COMBINING = 0b00000100;
    RESIZABLE = 0b00001000;
};

@discoverable
protocol Vmo {
    /// Kernel allocates physical frames and returns a capability handle
    Create(struct {
        size_bytes uint64;
        flags VmoFlags;
    }) -> (resource struct {
        status Status;
        vmo_handle handle:VMO;
    });

    /// Map physical frames into the caller's virtual address space (VMAR)
    Map(resource struct {
        vmar_handle handle:VMAR;
        vmo_handle handle:VMO;
        offset uint64;
        length uint64;
        protection_flags uint32; // READ | WRITE | EXECUTE
    }) -> (struct {
        status Status;
        mapped_virtual_address uint64;
    });

    /// Privileged Driver Call: Retrieve physical base address for DMA / Scanout
    GetPhysicalAddress(resource struct {
        vmo_handle handle:VMO;
    }) -> (struct {
        status Status;
        physical_address uint64;
    });
};

```

## End-to-End Flow: Display Swapchain with In-Kernel VMOs

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ 1. SCENED (Compositor)                                                      │
│    • Calls `svc #1` FIDL: `bexos.kernel.Vmo.Create`                         │
│      (flags = CONTIGUOUS_PHYSICAL | WRITE_COMBINING, size = 1920*1080*4)    │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ 1. Minted `vmo_handle`
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ 2. D0 MICROKERNEL (EL1 Buddy Allocator & Page Manager)                      │
│    • Allocates physically contiguous 4K/2M frames                           │
│    • Tracks ownership in the D0 handle table                                │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │
                   ┌───────────────────┴───────────────────┐
                   │ 2a. Map into scened                   │ 2b. Handoff to Driver
                   ▼                                       ▼
┌──────────────────────────────────────┐ ┌────────────────────────────────────┐
│ `scened` / Vello Render Target       │ │ D1 Display Driver (DRM / KMS)      │
│ • Maps VMO into address space        │ │ • Receives `vmo_handle` via FIDL   │
│ • Imports into `wgpu` / Vulkan       │ │ • Kernel verifies D1 capability    │
│ • Renders composite frame via compute│ │ • Driver extracts physical address │
│   shaders directly into target memory│ │ • Programs hardware CRTC scanout   │
└──────────────────────────────────────┘ └────────────────────────────────────┘

```

## Division of Responsibility

* **Microkernel (D0):** Owns physical frame allocation, buddy tables, contiguous memory blocks for DMA, IOMMU page tables, and capability handle isolation (`VMO` / `VMAR`).
* **Userspace Daemons & Drivers:** Perform layout constraints negotiation (e.g., matching pixel formats and stride alignments) over FIDL, but obtain the actual backing memory directly from kernel `VMO` primitives.

* Graphics drivers in BexOS split into two distinct userspace servers: the **D1 Display Controller Driver** (modesetting, hardware overlays, scanout via KMS/DRM) and the **D1 GPU Driver** (Vulkan / GPU command ring submission, e.g., VirtIO-GPU or Mali/Adreno).

The connection from driver hardware up to **Vello inside `scened`** operates via zero-copy shared VMOs negotiated over FIDL.

## The Graphics Driver FIDL Boundary

The display and GPU drivers do not use custom syscalls; they use the standard `svc #1` FIDL multiplexer.

```fidl
library bexos.hardware.display;

using bexos.kernel;

type PixelFormat = strict enum : uint32 {
    BGRA32 = 1;
    RGBA32 = 2;
    YUV420 = 3;
};

type BufferCollectionConstraints = struct {
    min_buffer_count uint32;
    pixel_format PixelFormat;
    alignment_bytes uint32;
};

@discoverable
protocol DisplayCoordinator {
    /// Negotiate hardware scanout buffer requirements
    AllocateBufferCollection(struct {
        constraints BufferCollectionConstraints;
    }) -> (resource struct {
        status bexos.kernel.Status;
        buffer_vmo handle:VMO;
        stride_bytes uint32;
    });

    /// Direct hardware plane presentation (Page Flip)
    SetScanoutBuffer(struct {
        vmo_id uint64;
        layer_index uint32;
        transform_flags uint32;
    }) -> (struct { status bexos.kernel.Status; });

    /// Register event port for hardware VSYNC notifications
    BindVsyncPort(resource struct {
        port_handle handle:PORT;
    }) -> (struct { status bexos.kernel.Status; });
};

```

## How Vello Binds to Driver-Allocated VMOs

Vello renders through `wgpu`. To make Vello render directly into the scanout buffer without CPU copies:

1. **Vulkan Device Context:** `scened` communicates with the D1 Vulkan GPU driver via a `wgpu_hal::vulkan` instance backed by the userspace driver library.
2. **External Memory Import:** When `scened` receives the swapchain `VMO` handle from the Display Driver FIDL call, `scened` imports the VMO into Vulkan as an `ExternalMemory` texture target (`VK_KHR_external_memory` backed by the microkernel VMO handle).
3. **Vello Render Target:** `scened` wraps this imported texture in a `wgpu::TextureView` and passes it to `vello::Renderer::render_to_texture()`.

```rust
// Inside scened render loop:
let target_texture = wgpu_device.create_texture_from_vmo(
    &swapchain_vmo,
    screen_width,
    screen_height
);

let mut scene = vello::Scene::new();

// 1. Taffy / Stylo calculate window clips and transforms
let (rect, transform) = compute_flatland_node_transform(node);
scene.push_layer(vello::kurbo::Rect::from_origin_size(rect.origin, rect.size));

// 2. Render blurred background if requested
if node.has_backdrop_blur() {
    vello_effects::dual_kawase_blur(&mut scene, &underlying_surface, radius);
}

// 3. Dispatch compute shaders directly into the hardware VMO
vello_renderer.render_to_texture(
    &wgpu_device,
    &wgpu_queue,
    &scene,
    &target_texture.create_view(&Default::default()),
    &vello::RenderParams {
        base_color: vello::peniko::Color::TRANSPARENT,
        width: screen_width,
        height: screen_height,
        antialiasing_method: vello::AaConfig::Msaa16,
    }
).expect("Vello render execution failed");

```

## VSYNC Presentation Flow

* **Step A (Render):** On the VSYNC deadline tick, `scened` evaluates layout in `taffy`, executes the composite pass in `vello`, and issues command buffer submission.
* **Step B (Submit):** `scened` calls `DisplayCoordinator.SetScanoutBuffer(vmo_id, ...)` over FIDL.
* **Step C (Scanout):** The D1 Display driver writes the base physical address of the VMO to the hardware display controller register (`CRTC_SCANOUT_ADDR`), achieving tear-free presentation with zero RAM buffer copies.
