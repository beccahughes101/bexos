# RFC 0062 current implementation

- Reviewed: 2026-09-12
- Design: [RFC 0062](README.md)

## Implementation summary

The first native UI component stack is implemented. SysUI and UserUI now build
retained Dioxus documents with shared `//lib/ui` primitives, the shared Dioxus
component forwards those documents to the native runner, and the runner resolves
Stylo stylesheets, projects supported layout properties into Taffy, and presents
the resulting scene through the existing Flatland/Vello/CPU renderer path.

Theme preferences are a real config package, `bexos.ui.theme`, delivered by
`prefsd` as the public `bexos.ui.theme.ThemeManager` protocol. The runner watches
that protocol, keeps document bytes retained per view, and redraws retained
documents when theme generation changes.

## Implemented behavior

- `//lib/ui/core`, `//lib/ui/button`, `//lib/ui/text_input`, and
  `//lib/ui/prelude` provide shared document-building helpers, component CSS,
  state bits for pseudo-class styling, and shell-facing widgets.
- `//lib/ui/theme` defines the theme preference schema and ships a prototxt
  config-only app archive. The package is included in the QEMU base image and
  storage preinstall set.
- `//lib/ui/runtime` is the trusted native document renderer. It validates the
  Dioxus document, installs user-agent, user, and author stylesheets into Stylo
  with their proper origins, maps the supported computed layout subset into
  Taffy, and emits validated scene batches.
- `//lib/dioxus_dom` document ABI version 2 adds stylesheet origins, raw author
  stylesheets, node attributes, and node state while retaining v1 decode
  compatibility.
- `lib/wasm_runtime/wit/bexos.wit` exposes `bexos:wasm/ui.submit-document`.
  The shared Dioxus component now decodes/configures/re-encodes documents and
  submits them to that native host interface.
- `services/wasm_runner` retains encoded document bytes and theme generations in
  UI migration state. Snapshot version 2 decodes older v1 state by treating it
  as scene-only state with no retained document.
- `services/prefsd` publishes `ThemeManager.GetTheme` and `WatchTheme`, returns
  read-only stylesheet VMOs, broadcasts theme changes after durable preference
  commits, and carries theme observers through heart transplant.
- Default SysUI and UserUI manifests consume the theme manager grant, and both
  shells render their chrome/login/launcher controls via the shared UI prelude.

## Current limits

This implementation intentionally covers the shell/UI substrate first. It does
not yet implement every future component named in the RFC (`modal`, `slider`,
rich text shaping, full image decode, complete CSS property projection, or a
standalone graphical theme editor). The native renderer draws text with a small
bounded vector-cell fallback and projects only the supported CSS/layout subset
needed by the current shells. Those are current implementation limits, not
changes to the long-term RFC design.

## Source map

- Shared components: [`lib/ui`](../../../lib/ui)
- Document ABI: [`lib/dioxus_dom/src/lib.rs`](../../../lib/dioxus_dom/src/lib.rs)
- Native renderer: [`lib/ui/runtime/src/lib.rs`](../../../lib/ui/runtime/src/lib.rs)
- Theme schema/package: [`lib/ui/theme`](../../../lib/ui/theme)
- Theme service protocol: [`idl/bexos/preferences/preferences.fidl`](../../../idl/bexos/preferences/preferences.fidl)
- Theme service implementation: [`services/prefsd/src/wire.rs`](../../../services/prefsd/src/wire.rs)
- Runner document retention/redraw: [`services/wasm_runner/src/host/ui.rs`](../../../services/wasm_runner/src/host/ui.rs)
- SysUI adoption: [`apps/sysui/src/service/render.rs`](../../../apps/sysui/src/service/render.rs)
- UserUI adoption: [`apps/userui/src/desktop/render.rs`](../../../apps/userui/src/desktop/render.rs)
