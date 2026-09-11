# RFC 0047: Scene compositor architecture

- Created: 2026-09-01T10:52:10-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

Scened manages the Flatland scene graph, composition, and input-coordinate dispatch. Shared buffers and separate rendering paths support direct presentation, GPU effects, and CPU fallback.

## Design overview

> Implementation status (2026-09-08): Shared atomic transactions, committed
> geometry, capability-backed embedding, input routing, Taffy layout, Stylo
> styling, Parley/Noto text, CPU effects, direct imported presentation,
> scheduled CPU accounting, shared Rust/C allocation counters, and the Vello/Venus
> worker are implemented.
> Native sustained input/performance fixtures pass on AArch64 and x86_64, with
> five workload windows each using 32 warmups and 256 measured completions.
> Nested Venus sustained validation passes workloads 1-4 through GPU composition
> and workload 5 through direct imported presentation at the supported 800×600
> fixture extent. The focused host selection passed 45/45 targets, and the
> graphical boot/kernel gate passed 7/7 targets including 1080p graphical boot on
> both architectures.
>
> Fresh validation and the consolidated schema 3 report are tracked in
> [scened repository validation](../../scened-validation.md) and
> `docs/scened-completion-2026-09-08.json`. CPU fallback budget misses are
> reported from TCG/software fixtures. Hardware VSYNC, zero-copy GPU composition,
> hardware 1080p/120 Hz acceptance, and `a11yd` remain future work. See
> [current scened](../../scened.md) for interfaces, commands, measurements
> and limits. The architecture below remains the full long-term design.

`scened` is the central Flatland-style 2D composition, presentation, and input-routing engine for BexOS. Operating in D2 userspace with real-time deadline scheduling, it links layout resolution, styling transforms, vector rendering, and input/accessibility dispatch into a zero-ambient-authority presentation pipeline.

## System Architecture & Pipeline

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ APPLICATION CLIENTS / SYSUI (Dioxus Native / Blitz / WASM)                  │
│ • Renders UI into client-owned VMOs via Vello / Vello-CPU                  │
│ • Streams Flatland transforms, clipping rects, and semantic node trees     │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ 1. FIDL Sessions over IPC Channels
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ SCENED (System Scene Graph & Composition Server)                            │
│                                                                             │
│  [ Layout & Styling Engine ]                                                │
│  • Taffy: Computes multi-window flexbox/grid layout and split-screen bounds │
│  • Stylo: Evaluates node styling, z-order, opacities, and clipping paths    │
│                                                                             │
│  [ Scene Topology & Hit-Testing ]                                           │
│  • Transforms global input coordinates to client-local coordinate space     │
│  • Provides view transform matrices to `a11yd` for global semantic mapping  │
│                                                                             │
│  [ Dual-Path Composition Engine ]                                           │
│  ├─► Direct Scanout Plane Path (Fast Path: Zero GPU overhead)               │
│  └─► Vello GPU Compute Path (Overlays, rounded corners, Kawase blur passes) │
└───────────────────┬─────────────────────────────────────┬───────────────────┘
                    │                                     │
                    │ 2. VMO Page Flip / Scanout Commit   │ 3. VSYNC Event
                    ▼                                     ▼
┌──────────────────────────────────────┐ ┌────────────────────────────────────┐
│ D1 DISPLAY CONTROLLER DRIVER (DRM)   │ │ D0 MICROKERNEL SCHEDULER           │
│ • Programs CRTC scanout base address │ │ • Real-Time Deadline thread        │
│ • Fires hardware VSYNC interrupt     │ │   ($C=2.5\text{ms}, D=7.5\text{ms},│
│                                      │ │    T=8.33\text{ms}$ @ 120Hz)       │
└──────────────────────────────────────┘ └────────────────────────────────────┘

```

## Core Flatland FIDL Protocol (`bexos.ui.scened`)

Applications create an isolated `FlatlandSession` to manipulate their local sub-tree without accessing adjacent application surfaces:

```fidl
library bexos.ui.scened;

using bexos.kernel;

type PixelFormat = strict enum : uint32 {
    BGRA32 = 1;
    RGBA32 = 2;
};

type ViewEffect = strict union {
    1: blur_radius_px float32;
    2: opacity float32;
    3: corner_radius_px float32;
};

@discoverable
protocol FlatlandSession {
    /// Create a transform or content node in the local scene graph
    CreateTransform(struct { node_id uint64; });

    /// Set 2D spatial properties without client-side redrawing
    SetTranslation(struct { node_id uint64; x int32; y int32; });
    SetScale(struct { node_id uint64; scale_x float32; scale_y float32; });
    SetClipBounds(struct { node_id uint64; width uint32; height uint32; });

    /// Attach visual effects (e.g. frosted glass backdrop blur behind panels)
    SetEffect(struct { node_id uint64; effect ViewEffect; });

    /// Bind a client-rendered VMO buffer to a node
    SetContent(resource struct {
        node_id uint64;
        buffer_vmo handle:VMO;
        stride_bytes uint32;
        format PixelFormat;
    });

    /// Atomically commit all topology mutations for the next frame
    Present(struct { presentation_time_ticks uint64; }) -> (struct {
        status bexos.kernel.Status;
    });
};

```

## Internal Module Decomposition

```
services/scened/
├── src/
│   ├── main.rs                 # Deadline loop setup, VSYNC port listener, IPC entry
│   ├── session.rs              # Per-client Flatland session isolate & capability checks
│   ├── scene/
│   │   ├── graph.rs            # Transform tree hierarchy, z-index resolution
│   │   ├── layout.rs           # Taffy layout container & screen inset manager
│   │   └── style.rs            # Stylo theme resolver & transform calculations
│   ├── render/
│   │   ├── pipeline.rs         # Direct scanout plane assignment vs GPU composition
│   │   ├── vello_backend.rs    # Vello compute pass, Kawase blur shaders, corner masks
│   │   └── vello_cpu_fallback.rs # CPU software rasterizer fallback for headless/VM
│   └── input/
│       ├── hit_test.rs         # Inverse matrix multiplication ($M_{\text{view}}^{-1}$)
│       └── router.rs           # Focus-isolated keyboard & gesture dispatch
└── BUILD.bazel

```

## Composition Engine: Fast-Path vs. Compute Shader Path

To balance visual quality with high frame-rate performance, `scened` dynamically selects the rendering path per frame:

### Direct Scanout Fast-Path (Zero GPU Overhead)

* Evaluated when full-screen apps or non-overlapping rectangular surfaces are active without opacity, corner clips, or backdrop filters.
* `scened` bypasses GPU rendering entirely and submits the client VMO directly to the hardware display controller via the `DisplayCoordinator` protocol.

### Vello GPU Compute Path

* Activated when windows overlap, have rounded corner masks, or when UI surfaces (such as the Control Center or Lock Screen) request backdrop blur effects.
* `scened` executes a multi-pass compute graph:
1. Composite lower surfaces into an offscreen scratch VMO.
2. Run a 2-pass downsampled Dual Kawase blur shader over the region.
3. Render the top-layer UI surface with anti-aliased sub-pixel clipping masks via `vello`.

### CPU Fallback Path

* When running on hardware without Vulkan compute support (or during headless early-boot), `scened` falls back to `vello_cpu` to perform rasterization and single-pass box blurs.

## Input & Coordinate Dispatch Pipeline

`scened` acts as the secure spatial arbiter for all pointer, touch, and keyboard interactions:

1. **Hardware Ingestion:** Reads normalized input events (`bexos.hardware.input`) from the input event hub.

### Spatial Coordinate Hit-Testing

* Uses `Taffy`-resolved bounding rects and `Stylo` 2D affine transform matrices ($M_{\text{view}}$) to determine the topmost visible client under the cursor/touch contact.
* Maps global coordinates $(X_g, Y_g)$ into view-local coordinates $(X_l, Y_l)$ using the inverse matrix:

$$\begin{pmatrix} X_l \\ Y_l \end{pmatrix} = M_{\text{view}}^{-1} \begin{pmatrix} X_g \\ Y_g \end{pmatrix}$$

3. **Focused Routing:** Dispatches keyboard events exclusively to the single focused channel handle, preventing un-focused background applications from capturing keystrokes.
4. **System Gesture Capture:** If a touch sequence matches system-reserved edge triggers (such as a swipe-up home gesture), `scened` claims event ownership and sends a `CANCEL` phase event to the underlying client channel.

## Implementation Sketch (`scened/src/main.rs`)

```rust
#![no_std]
extern crate alloc;

use alloc::collections::BTreeMap;
use bexos_fidl::hardware::display::DisplayCoordinatorClient;
use bexos_fidl::ui::scened::FlatlandSessionServer;

pub struct SceneDaemon {
    display_driver: DisplayCoordinatorClient,
    sessions: BTreeMap<u64, FlatlandSessionServer>,
    vsync_port: u64,
    vello_renderer: vello::Renderer,
}

impl SceneDaemon {
    pub async fn run(&mut self) {
        // Register deadline parameters with D0 microkernel (120Hz loop)
        self.set_deadline_scheduler(8_333, 2_500, 7_500);

        loop {
            // 1. Sleep until hardware VSYNC signal on kernel event port
            let _vsync_event = unsafe { bexos_sys::sys_port_wait(self.vsync_port) };

            // 2. Resolve layout with Taffy & evaluate transform matrices
            let render_plan = self.evaluate_scene_graph();

            // 3. Compose frame via Direct Scanout or Vello Compute
            if render_plan.can_direct_scanout() {
                self.display_driver.set_scanout_buffer(render_plan.primary_vmo_id()).await.ok();
            } else {
                let target_vmo = self.acquire_swapchain_vmo();
                self.vello_renderer.render_composite(&render_plan, &target_vmo);
                self.display_driver.set_scanout_buffer(target_vmo.id()).await.ok();
            }

            // 4. Yield execution back to general workloads
            bexos_sys::yield_now();
        }
    }

    fn evaluate_scene_graph(&self) -> RenderPlan {
        // Traverses active sessions, matches Stylo transforms & Taffy rects
        RenderPlan::default()
    }

    fn set_deadline_scheduler(&self, _period_us: u64, _runtime_us: u64, _deadline_us: u64) {}
    fn acquire_swapchain_vmo(&self) -> VmoHandle { VmoHandle::default() }
}

#[derive(Default)]
pub struct RenderPlan;
impl RenderPlan {
    pub fn can_direct_scanout(&self) -> bool { true }
    pub fn primary_vmo_id(&self) -> u64 { 0 }
}

#[derive(Default)]
pub struct VmoHandle;
impl VmoHandle { pub fn id(&self) -> u64 { 0 } }

```
