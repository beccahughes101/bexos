# RFC 0045: Input routing and event delivery

- Created: 2026-09-01T10:49:55-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

Input devices, the input service, and scened coordinate event delivery through capability-scoped interfaces. Committed scene geometry determines routing, with explicit focus and security boundaries.

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

Input in BexOS flows through a multi-tier pipeline designed around capability isolation, zero-allocation driver event streaming, and coordinate-space remapping inside `scened`.

## End-to-End Input Pipeline

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ 1. HARDWARE LAYER & D1/D2 DRIVERS                                           │
│  • USB / I2C / Bluetooth Controller Interrupts                              │
│  • Sandboxed D2 WASM / D1 Native Drivers parse raw HID Report Descriptors   │
│  • Emits normalized FIDL Input Events to a kernel FIFO channel              │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ FIDL: `bexos.hardware.input.Device`
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ 2. INPUT SERVICE / EVENT HUB (`inputd` or embedded in `scened`)             │
│  • Aggregates multiple physical input devices into a unified event stream   │
│  • Applies keymap translation (XKB/Unicode) & pointer acceleration curves   │
│  • Maintains global gesture state (multi-touch, pinch-to-zoom, fling)       │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ Hit-Testing & Viewport Topology
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ 3. SYSTEM COMPOSITOR (`scened` / Flatland)                                  │
│  • Hit-tests pointer/touch coordinates against the Taffy/Stylo scene graph  │
│  • Transforms global display coordinates into client-local coordinate space │
│  • Enforces focus routing (focused window receives keyboard events)         │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ Attenuated Per-View FIDL Stream
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ 4. APPLICATION SANDBOX (Dioxus Native / WASM Extension)                     │
│  • Receives local normalized events (`PointerDown { x, y }`, `KeyDown`)      │
│  • Triggers client UI event handlers and gesture recognizers                │
└─────────────────────────────────────────────────────────────────────────────┘

```

## Driver Layer: Normalizing Raw Hardware Reports

D1 (Native) or D2 (WASM) HID drivers receive device interrupts via kernel ports and decode raw HID descriptor byte streams into standard, normalized FIDL structs:

```fidl
library bexos.hardware.input;

type KeyboardEvent = struct {
    timestamp_ticks uint64;
    scan_code uint32;
    key_state strict enum : uint8 { PRESSED = 1; RELEASED = 2; REPEAT = 3; };
    modifiers uint32; // Shift, Ctrl, Alt, Super
};

type TouchContact = struct {
    tracking_id uint32;
    x float32; // Normalized [0.0, 1.0] across physical sensor
    y float32;
    pressure float32;
    major_axis float32;
};

type TouchEvent = struct {
    timestamp_ticks uint64;
    phase strict enum : uint8 { DOWN = 1; MOVE = 2; UP = 3; CANCEL = 4; };
    contacts vector<TouchContact>:10; // Supports up to 10-finger multi-touch
};

type MouseEvent = struct {
    timestamp_ticks uint64;
    relative_dx float32;
    relative_dy float32;
    scroll_delta_x float32;
    scroll_delta_y float32;
    button_mask uint32;
};

type InputReport = strict union {
    1: keyboard KeyboardEvent;
    2: touch TouchEvent;
    3: mouse MouseEvent;
};

```

## Hit-Testing & Coordinate Transformation in `scened`

Because client applications run in sandboxed processes and only see their own window surfaces, `scened` performs coordinate space conversion:

### Pointer & Multi-Touch Hit-Testing

1. When a `TouchEvent` arrives at screen coordinate $(X_g, Y_g)$, `scened` queries its active Flatland scene graph (layout computed via Taffy).
2. It identifies the topmost visible view containing $(X_g, Y_g)$.
3. `scened` applies the inverse of the view's 2D transform matrix:

$$\begin{pmatrix} X_{\text{local}} \\ Y_{\text{local}} \end{pmatrix} = M_{\text{view}}^{-1} \begin{pmatrix} X_g \\ Y_g \end{pmatrix}$$

4. The event is dispatched exclusively over that specific view's input channel.

### Keyboard Routing & Focus Ownership

* Keyboard events are never broadcast globally. `scened` routes them strictly to the single channel endpoint belonging to the currently focused window, preventing background keylogging attacks.

## Client View Input Interface (`bexos.ui.input.ViewInput`)

Applications connect to `scened` and listen on their dedicated input channel endpoint:

```fidl
library bexos.ui.input;

type PointerPhase = strict enum : uint8 {
    DOWN = 1;
    MOVE = 2;
    UP = 3;
    HOVER = 4;
    CANCEL = 5;
};

type ViewPointerEvent = struct {
    pointer_id uint32;
    local_x float32;
    local_y float32;
    phase PointerPhase;
    buttons uint32;
};

@discoverable
protocol ViewInputListener {
    OnPointerEvent(struct { event ViewPointerEvent; });
    OnKeyEvent(struct { event bexos.hardware.input.KeyboardEvent; });
};

```

## Gestures and System Shell Priority

* **System Gestures (Edge Swipes & Home Bar):** `scened` maintains reserved edge gesture recognizers (e.g., swipe up from bottom to go home, swipe down for control center). When a touch sequence matches a system gesture, `scened` claims touch ownership and sends a `CANCEL` phase event to the underlying application.
* **WASM Sandboxed Widgets:** Widgets embedded in `sysui` do not receive raw hardware events directly; `sysui` dispatches local click/touch coordinates through the declarative component hostcall boundaries.
