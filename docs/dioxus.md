# Dioxus WASM native UI

The [system and user shells](sysui.md) use this stack, including native child
views, window geometry, and transferable view state. The shared component
package is version 0.2; dependency selectors use version prefixes such as
`"0.2"`.

BexOS now has a first-party path for Rust UI services that run as WASI 0.2 components and present native 2D frames through scened. Applications keep their own state, lifecycle hooks, callbacks, and component model in the application component. The separately packaged `com.bexos.lib.dioxus` component is mounted at `/deps/com.bexos.lib.dioxus` and exports the versioned WIT instance `bexos:wasm/dioxus@1.0.0`.

The implementation uses an explicit BexOS scene/document ABI between WASM and the trusted native runner. Native GPU handles, shaders, VMOs, renderer caches, Stylo/Taffy state, and compositor resources stay in the runner and scened. Apps normally submit bounded Dioxus documents; the runner resolves CSS and layout natively, resolves CSS font stacks through `fontd`, shapes text with Parley, renders glyph IDs with the provider's mapped font data on the Vello/Venus path, and otherwise replays the same validated scene on the CPU. Direct scene submission remains available for compatibility and renderer-focused tests.

## Source layout

- `//lib/dioxus_guest`: guest SDK for Rust WASI components. It generates bindings for the `dioxus-app` world, exposes constants for the required shared package/export, and wraps view, asset, scene, input, presentation, backend, and close calls.
- `//apps/dioxus_shared`: installable `com.bexos.lib.dioxus` WASM component package. It exports `bexos:wasm/dioxus@1.0.0`, decodes document submissions, configures the target view, and forwards validated document bytes to the native UI host interface. Raw scene submission remains available for compatibility and focused renderer tests.
- `//lib/dioxus_dom`: value-only document tree and stable wire ABI. Version 2 carries stylesheet origins, raw author stylesheets, attributes, node state bits, text/image nodes, and listener flags while retaining v1 decode compatibility for older document bytes.
- `//lib/ui`: shared native UI primitives and runtime. `core`, `button`, `text_input`, and `prelude` build reusable Dioxus document nodes and CSS; `theme` defines the `bexos.ui.theme` preference package; `runtime` renders retained documents with Stylo origins and Taffy layout.
- `//lib/dioxus_scene`: stable scene encoding, validation, and CPU replay. It covers paths, fills and strokes, linear gradients, transforms, clips/layers, glyph runs, image commands, and shadows.
- `//lib/dioxus_render`: native conversion from validated scene batches into Vello layers for the GPU worker.
- `//apps/dioxus_demo`: installable sample app with a signal-style counter, editable text, scrolling, simple flex/grid-style layout, an image asset command, backend status text, and explicit checkpoint/restore hooks.
- `//services/wasm_runner`: component composition, UI hostcalls, Vello/Venus rendering, CPU fallback, Flatland presentation, and migration quiescence.
- `//services/appd`: package dependency resolution and runner startup handle delivery for application plus component payloads.

## Packaging and manifest contract

All manifests remain prototxt. A Dioxus app declares a normal WASM process and names the component import it expects the runner to compose:

```prototxt
package_name: "bexos.app.dioxus_demo"
name: "Dioxus WASM GPU demo"
package_version { major: 0 minor: 1 patch: 0 }
library_dependencies {
  package_name: "com.bexos.lib.dioxus"
  version_requirement: ">=0.1.0"
  mount_alias: "com.bexos.lib.dioxus"
  abi_version: 1
}
processes {
  name: "dioxus_demo"
  runner: "wasm"
  service: true
  lifecycle { update_strategy: HEART_TRANSPLANT }
  runner_options {
    [type.googleapis.com/bexos.app.WasmRunnerOptions] {
      path: "/pkg/bin/dioxus_demo.wasm"
      component_imports {
        package_name: "com.bexos.lib.dioxus"
        export_name: "bexos:wasm/dioxus@1.0.0"
        instance_name: "bexos:wasm/dioxus@1.0.0"
        abi_version: 1
      }
    }
  }
}
```

The shared component package declares its export as a WASM component, not a native shared object:

```prototxt
package_name: "com.bexos.lib.dioxus"
name: "BexOS Dioxus shared WASM UI library"
package_kind: LIBRARY
package_version { major: 0 minor: 1 patch: 0 }
library_exports {
  name: "bexos:wasm/dioxus@1.0.0"
  path: "/pkg/lib/dioxus.wasm"
  abi_version: 1
  kind: WASM_COMPONENT
}
```

`library_exports.kind` defaults to `NATIVE`, preserving existing ELF behavior. WASM runner options carry explicit `component_imports`; appd resolves each through the existing package/version machinery, validates that the selected export is `WASM_COMPONENT`, maps the application and dependency payloads into runner startup handles, and rejects missing exports, incompatible ABI versions, dependency cycles, excessive graph depth/count/bytes, and native payload substitution.

The runner composes application and shared components with pinned `wac-graph` before compiling the resulting component with Wasmtime/Pulley. Running instances retain the resolved dependency identities in launch and migration state. Compatible shared-library changes are observed on a new launch or a replacement that prepares a new component graph.

## Guest SDK usage

A Rust app imports `bexos_dioxus_guest`, opens a view from its granted Flatland and display resources, builds a scene with `bexos_dioxus_guest::scene`, and submits one complete batch per dirty frame:

```rust
use bexos_dioxus_guest::{scene, View};

let view = View::open(800, 600)?;
let batch = scene::SceneBatch {
    width: 800,
    height: 600,
    clear_rgba: [18, 22, 32, 255],
    commands: vec![scene::Command::Path(scene::PathCommand {
        points: vec![
            scene::Point { x: 24.0, y: 24.0 },
            scene::Point { x: 180.0, y: 24.0 },
            scene::Point { x: 180.0, y: 88.0 },
            scene::Point { x: 24.0, y: 88.0 },
        ],
        closed: true,
        paint: scene::Paint::Solid([67, 121, 255, 255]),
        stroke_width: 0.0,
    })],
};
view.submit(&batch)?;
```

`View::submit_document()` sends an encoded `bexos_dioxus_guest::dom::Document` to the shared component. The current document ABI supports stable node IDs, tags, classes, attributes, pseudo-state bits, inline declarations, stylesheet origins, raw author stylesheets, text nodes, image nodes, and listener flags for pointer, keyboard, text, wheel, and focus handling. The shared component forwards the document to the native runner, which installs system/user/author stylesheets into Stylo, projects the supported computed layout subset into Taffy, resolves declared family stacks with an Inter/JetBrains Mono baseline and Arabic/Devanagari fallbacks, emits Parley glyph runs in a validated scene batch, and retains the document for theme-driven redraws. Signed package-local font assets remain available to direct scene submissions. `View::submit()` remains available for direct scene tests.

`View::input()` returns view-local pointer, keyboard, Unicode text, wheel, and focus events drained from the Flatland session. `View::presentation()` returns accepted sequence, pending count, latch ticks, and the scene generation submitted by the app. `View::backend()` reports `gpu`, `gpu-initializing`, or `cpu` plus the last renderer failure when a GPU initialization or device error forced fallback.

## Grants and security boundary

The app must request `bexos.ui.scened.FlatlandSession` Public methods 1, 2, 8, 14, 15, 16, 20, and 24. Methods 1/2/8/14/15/16/20 cover node creation, root/content/presentation/input/status operations used by the runner; method 24 supplies a session-local viewport query so resize and scale changes do not require shell control. Document-rendering apps also request `bexos.fonts.FontProvider` methods 1 and 2. SysUI and UserUI declare that required grant in their prototxt manifests.

The app may request `bexos.hardware.display.DisplayCoordinator` `GpuTransport` methods 7 through 15. The runner checks authenticated service grants and method restrictions before creating a GPU-backed renderer. If those methods are absent or GPU initialization fails, the same scene is replayed on the CPU. Unsigned child sandboxes cannot receive hardware/display grants through the existing child policy, so they cannot gain graphics access by spawning under a Dioxus app.

The scene validator rejects malformed or unsupported batches before rendering. It bounds byte size, command count, path points, glyphs, asset IDs, view dimensions, layer nesting, finite coordinates, image opacity, and asset ownership. A failed scene does not publish a new frame; the compositor keeps the last valid presentation.

## Rendering and presentation behavior

The native runner allocates presentation VMOs privately, renders into mapped BGRA buffers, and submits them through the shared Flatland content and fence APIs. It keeps at most two outstanding releases per view and reuses a buffer only after the compositor signals release. GPU completion by itself is not treated as compositor retirement.

The GPU path reuses the native Venus worker and Vello renderer. It has a bounded queue and a migration drain check. CPU fallback uses `//lib/dioxus_scene` to replay the validated scene commands into the same surface format. The current GPU bridge retains the existing readback path from the Venus worker rather than requiring zero-copy GPU import.

The first shipped scene ABI is deliberately 2D and UI-shaped. It supports paths, colors, gradients, transforms, layers/clips, glyph runs, image commands, and shadows. CSS selector resolution and DOM/document interpretation are kept behind the shared component/native-runner boundary so the application ABI does not expose Rust pointers or callbacks. The checked-in document path uses Parley rather than the former 5×7 vector-cell lettering. Its font cache is keyed by provider face identity and backed by read-only mappings; OCI lookup and Servo integration remain future work behind the same provider interface.

## Heart transplant

The Dioxus demo opts into `HEART_TRANSPLANT` and checkpoints serializable model state: counter value, editable text, scroll offset, backend observation, and stable logical view data. Restore reconstructs the document tree and scene from that model rather than serializing engine stacks, renderer pointers, Vello caches, VMOs, or native handles.

The runner migration codec retains dependency identities, component payload identities, UI resource metadata, presentation sequence state, and renderer quiescence requirements. During replacement, appd prepares package resolution, component composition, compilation, and replacement stores before the final quiescence point. The source keeps handling input and presentation until the candidate is ready. The runner refuses cutover while presentation releases or GPU jobs are still unsafe to transfer; failed restore resumes the original app without duplicating input or presentation. Mapped font handles and engine caches are not authoritative migration data: after adoption, the retained document is redrawn and its font stack is resolved again through `fontd`.

## Verification status

The Dioxus implementation was written before expensive tests were run. The focused checks for this change include:

```sh
bazel run @rules_rust//:rustfmt
bazel build --platforms=//build/platforms:userspace_aarch64 //services/wasm_runner:wasm_runner //services/scened:scened //apps/dioxus_shared:dioxus_shared //apps/dioxus_demo:dioxus_demo
bazel test //lib/dioxus_dom:tests //lib/dioxus_scene:tests //lib/dioxus_render:tests //lib/wasm_abi:tests //lib/wasm_runtime:wasm_runtime_tests //services/wasm_runner:wasm_runner_tests //services/appd:appd_tests //services/scened:tests //services/scened:controls_tests //services/scened:input_config_tests //services/usersd:usersd_tests
bazel test --test_tag_filters= //testing/e2e/qemu/graphics:dioxus_smoke_aarch64
bazel test --test_tag_filters= //testing/e2e/qemu/graphics:dioxus_smoke_x86_64
```

Local verification on this branch has passed Rust formatting, the focused host suite, and the userspace AArch64 build. Host tests cover document encoding/validation, selector/declaration parsing, WASM-side layout/text projection, scene validation/rejection, CPU replay, Vello conversion, WASM runner option bounds, dependency composition with the actual demo app and separately packaged shared component, appd legacy/native compatibility, native substitution rejection, and component payload delivery to the runner.

The graphics QEMU smoke targets are checked in as `//testing/e2e/qemu/graphics:dioxus_smoke_aarch64` and `//testing/e2e/qemu/graphics:dioxus_smoke_x86_64`. They boot the graphical workstation image, launch `bexos.app.dioxus_demo`, and assert component graph composition, service instantiation, scened readiness, virtio GPU readiness, and the runner marker `wasm_runner: native UI first frame submitted backend=...`. Current local QEMU runs are blocked before Dioxus rendering is reached: the AArch64 run reaches scened and debugd but wedges in the debug app-launch proxy after probe connection, while the x86_64 run fails during graphics workstation pivot with `appd: boot failed: driver library cache VMO bexos.lib.crypto; pivot not completed`. Because both failures occur before the app can render, this branch does not claim QEMU Dioxus rendering as locally verified yet. Existing scened validation continues to distinguish software Vulkan functional evidence from physical-GPU acceleration; this change does not add new physical-GPU performance measurements.

## Localized retained documents

Source integration is present; the local implementation and acceptance plan
remain incomplete. See [RFC 0066 current gaps](rfcs/0066/CURRENT.md#current-gaps-in-the-approved-local-scope).

`lib/ui/i18n` supplies `LocaleContext`, `I18nProvider`, `use_locale(context)`, and
`t!(context, key, ...)`. The guest SDK polls the runner's native locale watch
before a frame and marks the existing document dirty on a new generation. State
such as input, counters, focus, scroll, and logical view IDs remains outside the
context. Stylo passes computed direction to Taffy; Parley receives language and
paragraph direction as part of its shaping cache key. The demo checkpoint now
also retains its view ID and accepts the previous checkpoint layout.

SysUI, UserUI, and the demo embed build-time English catalogs. They fall back to
English when another language is selected; multilingual test catalogs are built
separately. [Localization](localization.md) describes FTL support, Bazel rules,
CLI preferences, native/WASM ownership, and the current verification boundary.
