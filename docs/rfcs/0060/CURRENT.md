# RFC 0060: System and user UI — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0060](README.md)

## Implementation summary

The basic single-display SysUI/UserUI milestone is implemented, including authentication, a launcher, floating windows, lock/unlock, and logout.

## Implemented behavior

- Signed WASM shell packages use the shared Dioxus component. Appd assigns internal shell roles and session generations; normal launch cannot manufacture those grants.
- Appd/prefsd resolve system and user package selectors, honor locks, and fall back to bundled shells with diagnostics while preserving invalid preferences. Usersd authenticates before the user desktop is selected.
- The desktop enumerates apps, moves/resizes/focuses windows, and stops a package’s session processes on close. Compositor grants isolate visibility and input by session.
- Shells, appd, and scened checkpoint supported selection/session/window state. Existing fixtures cover selected replacement/rollback scenarios.

## Gaps and deviations

- One display and one active user session are supported. Multi-monitor, fast switching, widgets, tiling, graphical settings/setup, overlays, biometrics, and complete internationalized text/input remain missing.
- Current limits include 16 compositor views, 64 launcher entries, and 48 kernel process records; exited records can exhaust the boot’s process table. Windows are not independently closable within one package.
- The UI guide records full AArch64 evidence and a remaining complete x86 acceptance rerun after a timeout correction. Focused x86 replacement tests do not substitute for that missing full run.

## Sources and validation

Implementation and contract evidence: [apps/sysui/src](../../../apps/sysui/src), [apps/userui/src](../../../apps/userui/src), [services/appd/src/shell.rs](../../../services/appd/src/shell.rs), [services/appd/src/guest/shell.rs](../../../services/appd/src/guest/shell.rs), [services/scened/src/shell.rs](../../../services/scened/src/shell.rs), [kernel/core/src/runtime/mod.rs](../../../kernel/core/src/runtime/mod.rs), [idl/bexos/shell/session.fidl](../../../idl/bexos/shell/session.fidl).

Relevant test sources and Bazel targets: [apps/userui/src/desktop/tests.rs](../../../apps/userui/src/desktop/tests.rs), [services/appd/BUILD.bazel](../../../services/appd/BUILD.bazel), [testing/e2e/qemu/graphics/BUILD.bazel](../../../testing/e2e/qemu/graphics/BUILD.bazel).

Detailed guides and previously recorded validation: [sysui](../../sysui.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
