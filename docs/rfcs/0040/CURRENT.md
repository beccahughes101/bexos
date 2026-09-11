# RFC 0040: Compositor, system shell, and launcher — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0040](README.md)

## Implementation summary

The compositor and basic graphical shell are implemented, with the newer SysUI/UserUI split rather than the exact shell.bex plus launcher.bex topology.

## Implemented behavior

- Scened owns composition, view capabilities, focus, and display access. A UID-0 SysUI authenticates users; a selected per-user UserUI supplies desktop windows and the launcher.
- Appd grants shell roles internally, resolves configured packages, and binds session identity. Lock hides/revokes user input; logout terminates the session’s processes.
- Shell selection/session and compositor ownership have migration records, while each shell implements its own WASM service lifecycle.

## Gaps and deviations

- The separate pluggable launcher role and complete unified shell chrome in this RFC are not the actual package boundaries; RFC 0060 describes the selected implementation.
- Widgets, notification/quick-settings overlays, general permission/openers UI, multi-monitor support, and fast user switching remain missing.
- Basic shell state transfer does not imply arbitrary application window continuity, secure biometric prompts, or complete internationalized desktop input.

## Sources and validation

Implementation and contract evidence: [apps/sysui](../../../apps/sysui), [apps/userui](../../../apps/userui), [services/appd/src/shell.rs](../../../services/appd/src/shell.rs), [services/appd/src/guest/shell.rs](../../../services/appd/src/guest/shell.rs), [services/scened/src/shell.rs](../../../services/scened/src/shell.rs), [idl/bexos/shell/session.fidl](../../../idl/bexos/shell/session.fidl).

Relevant test sources and Bazel targets: [services/appd/BUILD.bazel](../../../services/appd/BUILD.bazel), [apps/userui/src/desktop/tests.rs](../../../apps/userui/src/desktop/tests.rs), [testing/e2e/qemu/graphics/BUILD.bazel](../../../testing/e2e/qemu/graphics/BUILD.bazel).

Detailed guides and previously recorded validation: [sysui](../../sysui.md), [scened](../../scened.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
