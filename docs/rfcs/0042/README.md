# RFC 0042: Flatland rendering, effects, and scheduling

- Created: 2026-08-31T15:35:04-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

Scened combines layout, styling, vector rendering, and imported surfaces for Flatland composition. The design allocates animation and effect responsibilities, defines fallbacks, and describes realtime scheduling.

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

`scened` uses Vello, Taffy, and Stylo for Flatland rendering and compositor effects. Together, these crates form a Rust-based, hardware-accelerated 2D pipeline for layout, scene graphs, and shader-driven effects such as backdrop blur.

## How Each Crate Maps to Flatland's Responsibilities

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ APPLICATION CLIENTS (SysUI, Apps, Widgets)                                  │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ FIDL: `bexos.ui.scened.Flatland`
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ SCENED (System Compositor)                                                  │
│                                                                             │
│  1. Layout & Constraints Engine (Taffy)                                     │
│     • Resolves multi-window flexbox/grid layout and split-screen rules      │
│     • Computes absolute pixel rects for shell surfaces and insets           │
│                                                                             │
│  2. Style & Transform Graph (Stylo / Flatland Scene Graph)                  │
│     • Manages window z-order, opacity layers, clipping paths, and matrix   │
│       transforms (translate, scale, rotate) without client redraws         │
│                                                                             │
│  3. Vector & Effects Render Engine (Vello over wgpu / compute)              │
│     • Rasterizes rounded window corners, shadows, and vector borders        │
│     • Executes backdrop blur passes (frosted glass) using compute shaders   │
│     • Composites client-provided surface VMOs onto display swapchain        │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ Hardware Presentation
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ DISPLAY ENGINE / SCANOUT (KMS / DRM / VirtIO-GPU)                           │
└─────────────────────────────────────────────────────────────────────────────┘

```

## Taffy in `scened` (Window & Shell Layout Engine)

* **Role:** Resolves window layout constraints, status-bar insets, desktop tiling splits, and floating modal alignments.
* **Why it fits:** Taffy is a pure-Rust, `no_std`-friendly Flexbox and CSS Grid layout engine. Rather than hand-coding rigid layout math in C/C++, `scened` can evaluate window docking, split views, and display insets (e.g., status bar cutouts or navigation docks) dynamically and deterministically.

## Stylo / Scene Graph in `scened` (Topology & Property Resolution)

* **Role:** Manages the Flatland node hierarchy, applying styling attributes, z-indexing, visibility state, and 2D transform matrices.
* **Why it fits:** Stylo (derived from Servo/Firefox) excels at high-speed rule matching and CSS style resolution. In `scened`, it allows declarative styling of window chrome (e.g., border radii, surface tint colors, dynamic window shadows, and transitions) driven by the active system theme.

## Vello in `scened` (Vector Rendering & Hardware Effects)

* **Role:** GPU-accelerated rendering of vector paths, window masks, and multi-pass composition effects.

### How it handles Flatland rendering

* **Window Clipping & Rounded Corners:** Renders smooth, anti-aliased sub-pixel corner radii and clip masks directly via GPU compute without tessellation artifacts.
* **Backdrop & Material Blurs (Frosted Glass):** When a UI node (like the control center or lock screen shade) requests a backdrop blur, Vello's compute pipeline samples the underlying composite buffer, runs a downsampled Gaussian/Kawase blur shader pass, and blends the overlay in a single GPU pass.
* **Surface Compositing:** Blits client-provided double-buffered VMOs into their final clipped, transformed screen rectangles.

## Composition Optimization Strategy

To maintain solid 60/120 FPS performance in `scened`:

1. **Direct Scanout Fast-Path (No Vello Pass):** When rendering fullscreen opaque apps or non-overlapping rectangular surfaces without blur/effects, `scened` bypasses GPU rendering entirely and assigns the client VMOs directly to hardware KMS/DRM display planes (zero GPU overhead).
2. **Vello GPU Pass (Active UI Effects):** When windows overlap, feature transparency, use rounded corner clips, or require backdrop frosted-glass blurs, `scened` runs the composite pass through Vello.
3. **Software Fallback (`vello_cpu`):** In headless setups or VM instances lacking Vulkan compute support, `scened` executes the layout and composition pass using `vello_cpu` to guarantee identical rendering output.

4. In a Flatland-style 2D composition model, **animations and blur effects live across three distinct boundaries** depending on whether they are *local to an application*, *global window/surface transitions*, or *cross-surface background materials (frosted glass)*.

## Division of Responsibilities

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ 1. APP / IN-PROCESS (Dioxus Native / Vello / Blitz)                         │
│    • Local widget animations (button press, list re-ordering, text fades)  │
│    • Foreground element blur (e.g. drop shadows inside the app view)        │
│    • Evaluated entirely inside the App's WASM sandbox                       │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ Mapped Surface VMO + Transform Table
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ 2. COMPOSITOR TOPOLOGY (`scened` / Flatland Scene Graph)                    │
│    • Hardware Surface Placement (2D affine matrix: translate, scale, crop)  │
│    • Window open/minimize/slide transitions (driven by SysUI or scened)     │
│    • Direct Display Controller Overlay Assignment (Scanout planes)          │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ If Background Blur / Effect is bound
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ 3. COMPOSITOR EFFECT PIPELINE (`scened` GPU Render Pass)                    │
│    • Backdrop Blur (Frosted glass behind SysUI / Control Center)            │
│    • Composition Shaders (Dual Kawase / Downsample-Upsample Gaussian passes)│
│    • Composites lower-z surfaces into an offscreen scratch VMO              │
└─────────────────────────────────────────────────────────────────────────────┘

```

## Where Animations Live

### A. Internal App Animations $\longrightarrow$ Client-Side (Dioxus / Vello)

* Animations that modify internal UI state (e.g., a progress bar filling, a list item expanding, or a button hover) live **inside the client application**.
* The application runs its animation ticker, re-evaluates layout via `taffy`, draws the scene via `vello` (GPU) or `vello_cpu` (fallback), and presents the resulting buffer to `scened`.

### B. Window & Surface Transitions $\longrightarrow$ Compositor Scene Graph (`scened`)

* When an app window slides in, scales up, or changes opacity (e.g., swiping down the Quick Settings shade or switching apps):
* The app **does not redraw its pixels**.
* The app's buffer stays static in its mapped VMO.
* `sysui` or the window manager mutates the node's **2D transform matrix** (`SetTranslation`, `SetScale`, `SetOpacity`) inside `scened` via FIDL.
* `scened` applies this affine transform at VSYNC time using GPU hardware planes or a single blit shader.

## Where Blur Effects Live

Blur is split based on what is being blurred:

### A. In-App Blur (Drop shadows, blurred images) $\longrightarrow$ App / Vello

* Handled entirely by the application's Vello/Vello-CPU pipeline before writing to the swapchain VMO.
* `scened` is unaware of these effects; it only receives the final rendered image.

### B. Backdrop / Frosted Glass Blur $\longrightarrow$ Compositor (`scened`)

* When `sysui` opens a semi-transparent control center that needs to blur whatever running apps or wallpapers are underneath:
* **Why the app cannot do this:** An application process does not have capability access to read adjacent window VMOs due to microkernel sandboxing.

#### How `scened` handles it

1. `sysui` attaches an effect metadata attribute to its Flatland View node: `SetBackdropFilter(Blur { radius: 24.0 })`.
2. During the VSYNC render pass, `scened` renders all layers *below* the `sysui` node into a temporary offscreen GPU texture.
3. `scened` executes a **Fast Dual Kawase Blur** or downsampled Gaussian compute pass over that texture.
4. `scened` blends the `sysui` panel on top of the blurred background before scanning out to the display.

## Flatland FIDL Interface with Effect Modifiers

```fidl
library bexos.ui.scened;

using bexos.kernel;

type BlurEffect = struct {
    radius_px float32;
    tint_color array<uint8, 4>; // RGBA overlay tint
};

type ViewEffect = strict union {
    1: blur BlurEffect;
    2: opacity float32;
};

@discoverable
protocol FlatlandSession {
    /// Create a transform node in the 2D scene graph
    CreateTransform(struct { node_id uint64; });

    /// Update 2D position/scale without client redrawing
    SetTranslation(struct { node_id uint64; x int32; y int32; });
    SetScale(struct { node_id uint64; scale_x float32; scale_y float32; });

    /// Attach backdrop effects (processed by compositor GPU passes)
    SetBackdropEffect(struct {
        node_id uint64;
        effect ViewEffect;
    });

    /// Atomically commit the frame scene changes
    Present(struct { presentation_time uint64; }) -> (struct {
        status bexos.kernel.Status;
    });
};

```

## Performance & Fallback Strategy

* **Hardware Overlay Fast-Path:** When no blur is present, `scened` assigns flat rectangles directly to hardware display controller (KMS/VDC) scanout planes, achieving **zero GPU composition overhead**.
* **GPU Compute Shader Path (`vello` / `wgpu`):** When backdrop blur is requested, `scened` switches to GPU composition for the affected bounding box using a 2-pass downsampled Kawase blur.
* **Software Fallback Path (`vello_cpu`):** In headless or VM environments lacking hardware acceleration, `scened` skips expensive live multi-pass blurs and falls back to a single-pass box-blur or a translucent tint to preserve 60fps CPU execution.

The design favors a Rust vector rendering stack, such as **`vello`** for GPU compute paired with **`tiny-skia`** for software fallback, over full C++ Skia.

## Tradeoffs of full C++ Skia integration

* **Massive C++ Build Footprint:** Skia requires a large C++ build system (GN/Ninja), additional toolchains, C++ runtimes, and platform shims within BexOS's Rust/FIDL architecture.
* **Heavy Binary Size:** Linking full Skia adds **15MB to 30MB+** to userspace service and UI runtime binaries.
* **Memory Safety & Sandbox Isolation:** Parsing complex vector paths, SVGs, and glyph layouts in a large C++ codebase increases vulnerability surface area inside the display pipelines.

## Recommended Rust graphics stack

A modular, pure-Rust graphics pipeline aligns with the Dioxus Native (Blitz) and WASM app ecosystem:

| Layer | Recommended Engine | Role & Why |
| --- | --- | --- |
| **GPU Vector Renderer** | **`vello`** (via `wgpu`) | High-performance, GPU-compute-driven 2D renderer written in Rust. Renders complex Bezier curves, anti-aliasing, and blurs directly on the GPU without CPU tessellation bottlenecks. |
| **Software CPU Fallback** | **`tiny-skia`** | Minimal, pure-Rust software rasterizer (<500KB). Ideal for early boot (`splashd`), headless rendering, or environments lacking hardware GPU acceleration. |
| **Layout & Styling** | **`taffy` + `stylo`** | High-speed Flexbox/Grid layout and CSS style resolution (used natively by Dioxus Blitz). |
| **Text Layout & Shaping** | **`parley` + `rustybuzz`** | Pure-Rust HarfBuzz port and text-shaping engine. Zero C++ dependencies while handling complex scripts, bidi, and font metrics. |

## Architecture in Practice

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ APPLICATION / SYSUI (Dioxus Native / Blitz DOM)                             │
├─────────────────────────────────────────────────────────────────────────────┤
│ 1. Layout & Styling: `taffy` (Flexbox/Grid) + `parley` (Text Shaping)      │
│ 2. Vector Draw Scene: Emits unified 2D draw command list                   │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ Render Path Selection
                   ┌───────────────────┴───────────────────┐
                   ▼                                       ▼
┌──────────────────────────────────────┐ ┌────────────────────────────────────┐
│ Primary Path: `vello` (GPU Compute)  │ │ Fallback Path: `tiny-skia` (CPU)   │
│ • Runs over `wgpu` / Vulkan / VirtIO │ │ • Pure Rust software rasterizer    │
│ • Zero CPU tessellation overhead     │ │ • Used by `splashd` & headless VMs │
└──────────────────┬───────────────────┘ └─────────────────┬──────────────────┘
                   │                                       │
                   └───────────────────┬───────────────────┘
                                       │ Blits to double-buffered VMO
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ SYSTEM COMPOSITOR (`scened`)                                                │
└─────────────────────────────────────────────────────────────────────────────┘

```

## Key Benefits

* **Unified Rust Ecosystem:** Native interoperability with Dioxus Native, Blitz, and the `no_std`/`std` userspace abstractions.
* **Target Portability:** Compiles to native x86_64, AArch64, and WebAssembly (`wasm32-wasip2`) with standard `cargo build`.
* **Zero C++ Interop Overhead:** Eliminates fragile C-FFI bindings, custom GN wrappers, and dynamic C++ runtime loading across WASM app sandboxes.

* `scened` should use **deadline-scheduled real-time threads (SCHED_DEADLINE / Earliest Deadline First)** rather than being pinned to an exclusive dedicated CPU core.

## Drawbacks of dedicated-core pinning

* **Excessive Core Idling:** At 60Hz (16.6ms frame budget) or 120Hz (8.3ms frame budget), an efficient compositor like `scened` typically only spends **0.5ms to 2ms per frame** doing clip-tree traversals and dispatching GPU draw commands/swapchains. Pinning it to an exclusive core wastes 85–95% of that core’s compute capacity that other threads (WASM app runners, `netstack`, background compile jobs) could use.
* **Core Count Scalability:** On low-end hardware (dual-core IoT/mobile devices or 2-vCPU VMs), dedicating a full core to the display compositor permanently halves available general-purpose compute for user applications.
* **Power & Thermal Penalty:** Preventing a core from entering low-power idle states (`WFI` / C-states) increases power draw on mobile and battery-powered devices.

## Recommended realtime deadline scheduling

Instead of hard CPU affinity, configure `scened`'s rendering loop as a **Deadline Real-Time Thread** inside the D0 microkernel scheduler:

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ D0 DEADLINE SCHEDULER PARAMETERS FOR `scened`                               │
├─────────────────────────────────────────────────────────────────────────────┤
│ • Period ($T$):       8,333 µs (for 120Hz) or 16,666 µs (for 60Hz)          │
│ • Runtime ($C$):      3,000 µs (Guaranteed CPU execution slice per frame)   │
│ • Deadline ($D$):     7,500 µs (Target completion prior to VSYNC interrupt) │
└─────────────────────────────────────────────────────────────────────────────┘

```

```
                   VSYNC Interrupt (Timer Event)
                               │
                               ▼
  [ 0ms ] ────────────────────────────────────────────────────────── [ 8.33ms ]
  │◄─── scened runs (1.2ms) ───►│◄────────── General Workload ───────────────►│
  │ • Samples input queues      │ • WASM App processing                       │
  │ • Updates scene graph       │ • Netstack packet processing                │
  │ • Submits GPU Command Buffer│ • Background disk I/O                       │
  │ • Sleeps until next VSYNC   │                                             │

```

## Scheduling implementation

### Hardware VSYNC Pacing via Port Event

* The display driver triggers a VSYNC interrupt, or a high-precision hardware timer fires at the display refresh cadence ($1/60\text{s}$ or $1/120\text{s}$).
* The event posts a signal to `scened`'s event port.

### D0 Scheduler Guarantees

* When the VSYNC event arrives, the D0 scheduler immediately preempts non-real-time user/app threads and context-switches to `scened`.
* `scened` executes its fixed composition pass, emits the presentation buffer to the display engine, and voluntarily yields back via `sys_port_wait` or `yield_now()`.

### Hybrid Affinity (Big/LITTLE & Asymmetric SoCs)

* While `scened` should not have an *exclusive* core, its **affinity mask** can be set to prefer a mid/big performance core on heterogeneous ARM big.LITTLE / DynamIQ chipsets to ensure consistent frame execution times without stalling on energy-efficiency cores.
