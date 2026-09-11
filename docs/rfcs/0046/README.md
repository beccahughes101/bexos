# RFC 0046: Accessibility and semantic interfaces

- Created: 2026-09-01T10:50:35-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

Applications publish incremental semantic trees for accessibility tools. A11yd and scened coordinate geometry, actions, and input under capability and privacy policies.

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

Accessibility in BexOS works through a dedicated semantic tree pipeline that mirrors the visual scene graph in `scened` while enforcing strict capability boundaries, semantic node privacy, and screen reader / assistive technology isolation.

## System Architecture & Accessibility Pipeline

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ APPLICATION / SYSUI (Dioxus Native / Blitz / WASM)                          │
│ • UI Tree generates an Accessibility Semantic Node Tree (`AccessKit` model) │
│ • Declares roles (Button, Heading, Checkbox), labels, values, and actions   │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ FIDL: `bexos.accessibility.Semantics`
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ ACCESSIBILITY SERVICE / BROKER (`a11yd`)                                    │
│                                                                             │
│  [ Unified Global Semantic Graph ]                                          │
│  • Stitches isolated app semantic trees into a single global hierarchy      │
│  • Maps spatial coordinates using transform matrices provided by `scened`   │
│  • Tracks active accessibility focus / virtual cursor                       │
└───────────────────┬─────────────────────────────────────┬───────────────────┘
                    │                                     │
                    │ Query / Action Stream               │ Event Dispatch
                    ▼                                     ▼
┌──────────────────────────────────────┐ ┌────────────────────────────────────┐
│ ASSISTIVE TECH APPS / EXTENSIONS     │ │ SYSTEM COMPOSITOR (`scened`)       │
│ • Screen Reader (Text-to-Speech)     │ │ • Renders high-contrast focus rings│
│ • Switch Access / Head Tracker       │ │ • Magnifier viewport transform     │
│ • Braille Display Driver             │ │ • Color inversion / Color filters  │
└──────────────────────────────────────┘ └────────────────────────────────────┘

```

## The Semantic Protocol (`bexos.accessibility.semantics`)

Instead of parsing raw pixels or scraping DOM internals, client applications compile an **Accessibility Node Tree** (compatible with standards like `AccessKit`) and stream semantic mutations over FIDL:

```fidl
library bexos.accessibility;

type Role = strict enum : uint16 {
    UNKNOWN = 0;
    BUTTON = 1;
    CHECK_BOX = 2;
    TEXT_INPUT = 3;
    HEADING = 4;
    STATIC_TEXT = 5;
    IMAGE = 6;
    SLIDER = 7;
    LIST = 8;
    LIST_ITEM = 9;
};

type Action = strict enum : uint16 {
    DEFAULT = 1;        // Click / Activate
    FOCUS = 2;
    BLUR = 3;
    SET_VALUE = 4;
    SCROLL_FORWARD = 5;
    SCROLL_BACKWARD = 6;
};

type Rect = struct {
    x float32;
    y float32;
    width float32;
    height float32;
};

type SemanticNode = struct {
    node_id uint32;
    role Role;
    bounds Rect;             // Local coordinate space
    label string:256;        // Spoken accessibility text
    value string:256;        // Current value (e.g., "75%")
    children vector<uint32>:64;
    supported_actions uint32;// Bitmask of supported `Action` values
};

@discoverable
protocol SemanticTreeBridge {
    /// App registers its semantic tree root
    RegisterView(resource struct {
        view_token handle:CHANNEL;
    });

    /// App commits incremental updates to its semantic tree
    CommitTreeUpdate(struct {
        nodes_added_or_updated vector<SemanticNode>:128;
        nodes_removed vector<uint32>:128;
        root_node_id uint32;
    });

    /// `a11yd` triggers an action on behalf of user
    -> OnPerformAction(struct {
        node_id uint32;
        action Action;
        action_data string:256;
    });
};

```

## Coordination Between `a11yd` and `scened`

### Global Coordinate Mapping

`scened` provides `a11yd` with the 2D affine transformation matrices for all active application surfaces. `a11yd` multiplies client-local node bounds ($X_{\text{local}}, Y_{\text{local}}$) by the surface's matrix to resolve exact physical screen positions for focus indicators and magnification.

### Visual Focus Rings

When a user navigates via screen reader or switch controls, `a11yd` commands `scened` to draw a dedicated hardware overlay focus border around the target element without disturbing client window pixels.

### Magnification & Visual Filters

Fullscreen magnification and color correction (grayscale, high-contrast, protanopia/deuteranopia filters) are executed as single GPU compute shader passes inside `scened`'s final scanout composition pipeline.

## Security, Privacy & Permission Guardrails

Because accessibility APIs can potentially inspect screen text and simulate user clicks, BexOS applies capability constraints:

* **No Ambient Global Scraping:** Standard third-party apps cannot connect to `a11yd` or read adjacent application semantic trees.
* **Privileged Permission Tier:** Only apps signed by trusted providers holding the manifest permission `bexos.permission.ACCESSIBILITY_SERVICE` (e.g., first-party screen readers, switch control daemons) can bind to the global semantic reader protocol.
* **Protected Password Fields:** Nodes marked with role `TEXT_INPUT` and property `is_password = true` redact raw text strings in the semantic tree, exposing only token counts (e.g., `"6 characters entered"`) to prevent credential theft.

## How Dioxus / WASM Apps Emit Semantics

Inside Dioxus Native (using Blitz and AccessKit integration), accessible properties are declared inline in the component template:

```rust
rsx! {
    button {
        aria_role: "button",
        aria_label: "Send Message",
        onclick: move |_| send_chat_message(),
        "Send"
    }
}

```

The runtime compiles this element into an entry in the local `SemanticNode` table, tracks its layout box via `Taffy`, and emits an incremental update over the view's `SemanticTreeBridge` channel when rendered.
