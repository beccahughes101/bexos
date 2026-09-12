# RFC-0042: Native UI Component Architecture, Stylo Integration, and Multi-Tier Styling

* **Author:** BexOS Platform & UI Working Group
* **Status:** Proposed
* **Target Subsystems:** `libs/ui`, `sysui`, `userui`, `prefsd`, `scened`
* **Applicability:** Dioxus Native WASM Apps, Host UI Runners, System Services

---

## 1. Summary

This RFC establishes the presentation runtime for BexOS native applications. It defines:

1. A modular, package-per-component repository layout under `//libs/ui/<component>` built with Bazel.
2. The integration of **Stylo** (Firefox/Servo parallel CSS engine) within the native host runner to compute styles for headless Dioxus RSX/DOM trees.
3. A three-tier CSS cascade architecture (**System Agent**, **User/MDM Preference**, **App Author**) that leverages standard CSS specificity and `!important` semantics for system-wide theming and mandatory accessibility.
4. An IPC channel driven by `prefsd` to deliver dynamic, zero-restart theme updates via read-only Virtual Memory Objects (`VMO`).

---

## 2. Motivation

BexOS applications run in sandboxed D2 userspace jobs as native Rust/WASM binaries. Historically, desktop microkernel platforms have oscillated between two extremes:

* **Ad-hoc Custom DSLs:** Forcing developers to define geometry and styling in custom Rust macros or JSON configs, breaking compatibility with standard UI widgets and preventing shared theming.
* **Heavy Embedded Web Engines:** Spinning up a full browser instance (Chromium/WebKit) per app, incurring severe memory footprints and undermining the microkernel's performance guarantees.

By using **Dioxus** for component state management, **Stylo** for parallel CSS cascading, **Taffy** for Flexbox/Grid layout, and **Vello** for GPU vector rasterization, BexOS provides standard CSS styling capabilities with native microsecond performance, zero DOM bloat, and unified design token propagation.

---

## 3. Detailed Design

### 3.1 Repository Layout (`//libs/ui`)

UI primitives reside under `//libs/ui/` as granular Bazel targets to ensure fine-grained incremental builds, hermetic caching, and minimal dead code inclusion in compiled WASM artifacts.

```
//libs/ui/
├── BUILD.bazel
├── core/                       # Design tokens, theme primitives, Stylo shims
│   ├── BUILD.bazel
│   └── src/
│       ├── tokens.rs           # Token constants and CSS variable names
│       ├── provider.rs         # Root ThemeProvider Dioxus context
│       └── accessibility.rs    # Focus rings, ARIA/semantics bridges
├── prelude/                    # Meta-crate re-exporting the entire design system
│   ├── BUILD.bazel
│   └── src/lib.rs
├── button/                     # Discrete component package
│   ├── BUILD.bazel
│   ├── src/
│   │   ├── lib.rs              # Dioxus component logic & RSX
│   │   └── style.css           # Default component author stylesheet
│   └── tests/
├── text_input/
├── modal/
└── slider/

```

#### Bazel Rule Definition (`//libs/ui/button/BUILD.bazel`)

```python
load("@rules_rust//rust:defs.bzl", "rust_library", "rust_test")

rust_library(
    name = "button",
    srcs = glob(["src/**/*.rs"]),
    compile_data = ["src/style.css"],
    visibility = ["//visibility:public"],
    deps = [
        "//libs/ui/core",
        "@crates//:dioxus",
    ],
)

rust_test(
    name = "button_test",
    crate = ":button",
)

```

---

### 3.2 Headless Execution & Rendering Pipeline

Dioxus runs decoupled from browser APIs. The host runner bridges Dioxus elements to Stylo, computes layout via Taffy, and presents frames directly to `scened`.

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ DIOXUS WASM GUEST                                                           │
│ Emits RSX Node Hierarchy: `<button class="bex-btn primary">Save</button>`   │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ Shared Memory / Linear Memory
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ HOST RUNNER / STYLO STACK (D2 Native Sandbox)                               │
│                                                                             │
│  1. `stylo_core`: Constructs Gecko/Servo-compatible element tree proxy.     │
│  2. Cascade Calculation: Parallel selector matching across thread pool.    │
│  3. Layout Pass: Maps computed properties into Taffy (Flexbox/Grid).        │
│  4. Rasterization: Emits vector draw paths to Vello (Vulkan/Venus).         │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ Present Rendered Buffer
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ `scened` (Hardware Compositor via VMO Ring)                                 │
└─────────────────────────────────────────────────────────────────────────────┘

```

1. **Element Mapping:** Dioxus Virtual DOM mutations update an internal, memory-compact element graph that implements Stylo’s `TElement` and `TNode` traits.
2. **Parallel Cascade Resolution:** Stylo matches CSS selectors against classes, element IDs, attributes, and pseudo-classes (`:hover`, `:active`, `:focus`) concurrently using Rayon-style work stealing.
3. **Layout & Raster:** The resolved CSS properties (e.g., `display: flex`, `padding: 12px`, `background-color: var(--bex-accent)`) feed directly into Taffy for coordinate calculation and Vello for vector rasterization.

---

### 3.3 The Three-Tier Styling Cascade

Stylo natively implements the W3C Cascade Specification. BexOS models system, user, and application styles by organizing stylesheets into three distinct origins:

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ 1. SYSTEM AGENT STYLESHEET (Origin: UserAgent)                              │
│ Path: `/system/data/themes/base.css` (Read-only System BootFS)              │
│ • Canonical layout reset and baseline element rules                         │
│ • Default platform design tokens (`--bex-color-*`, `--bex-radius-*`)       │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ Overridden by
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ 2. USER PREFERENCE THEME (Origin: User)                                     │
│ Path: Managed by `prefsd` (`/data/users/<uid>/prefs/theme.css`)             │
│ • Custom accent colors, wallpaper-extracted palettes, font scaling          │
│ • Accessibility rules with `!important` overriding app-level styling        │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ Overridden by (except !important)
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ 3. APPLICATION STYLESHEET (Origin: Author)                                  │
│ Path: Bundled inside `.bex` application archive                             │
│ • App-specific UI components (`.calc-display`, `.video-timeline`)           │
│ • References global tokens via `var(--bex-*)`                               │
└─────────────────────────────────────────────────────────────────────────────┘

```

#### Precedence and `!important` Semantics

Per CSS Cascade specifications implemented by Stylo:


$$\text{User } (!\text{important}) > \text{Author } (!\text{important}) > \text{Author (Normal)} > \text{User (Normal)} > \text{UserAgent}$$

This mathematical certainty enables **Mandatory Accessibility Overrides**:
If an accessibility preference requires a high-contrast ratio or large typography, `prefsd` injects declarations with `!important`:

```css
/* Injected by prefsd for High-Contrast Mode */
* {
    font-family: "Atkinson Hyperlegible", sans-serif !important;
    font-size-adjust: 1.25 !important;
}

:root {
    --bex-bg-surface: #000000 !important;
    --bex-text-primary: #ffffff !important;
    --bex-border-width: 2px !important;
}

```

No application-level author styles can override these rules, guaranteeing platform accessibility compliance across third-party apps.

---

### 3.4 Dynamic Updates via `prefsd` and Shared VMOs

To support dynamic theme updates (e.g., toggling dark mode or changing accent colors) without restarting applications:

```
┌──────────┐                                                   ┌──────────────┐
│ `prefsd` │                                                   │ Host UI App  │
└────┬─────┘                                                   └──────┬───────┘
     │                                                                │
     │ 1. User updates theme in Settings                              │
     │ 2. Compile CSS into compact binary representation              │
     │ 3. Allocate read-only VMO (`theme_vmo`)                        │
     │                                                                │
     │ 4. Broadcast `OnThemeChanged(theme_vmo, generation)`           │
     ├───────────────────────────────────────────────────────────────►│
     │                                                                │ 5. Map VMO
     │                                                                │ 6. Replace User Sheet
     │                                                                │    in Stylo Context
     │                                                                │ 7. Mark Root Restyle Dirty
     │                                                                │ 8. Trigger Frame Redraw
     │                                                                │    (Zero State Loss)

```

#### FIDL Definition (`bexos.ui.theme`)

```fidl
library bexos.ui.theme;

using bexos.kernel;

type ColorScheme : uint8 {
    LIGHT = 1;
    DARK = 2;
    HIGH_CONTRAST = 3;
};

type ThemeMetadata = struct {
    generation uint64;
    color_scheme ColorScheme;
    text_scale f32;
};

@discoverable
protocol ThemeObserver {
    /// Pushed by prefsd when global theme or accessibility settings change
    OnThemeChanged(resource struct {
        metadata ThemeMetadata;
        stylesheet_vmo zx.Handle:VMO;
        stylesheet_len uint64;
    }) -> () error bexos.kernel.Status;
};

```

---

## 4. Implementation Plan

### Phase 1: Core Layout and Tokens

* Implement `//libs/ui/core` defining standard token schemas (`--bex-space-*`, `--bex-radius-*`, `--bex-color-*`).
* Stand up `//libs/ui/button` and `//libs/ui/text_input` implementing Dioxus RSX wrappers backed by default `style.css`.

### Phase 2: Host Stylo Integration

* Embed the Stylo style resolution crate into the native WASM/Dioxus runner harness.
* Wire the computed layout properties from Stylo into Taffy layout structures and hook drawing primitives to Vello.

### Phase 3: Cascade Delivery via `prefsd`

* Implement `ThemeObserver` in the host runner event loop.
* Extend `prefsd` to assemble the dynamic `theme.css` user stylesheet into a shared memory VMO and broadcast lifecycle updates across registered applications.