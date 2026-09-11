# RFC 0046: Accessibility and semantic interfaces — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0046](README.md)

## Implementation summary

Scened implements privileged accessibility-related geometry/display controls, but no complete accessibility semantics service or assistive application stack exists.

## Implemented behavior

- The compositor owns capability-backed view references and committed geometry. Private controls support focus rings, display transforms, and authorized injection/focus operations.
- Control state and view generations are preserved through compositor migration, providing a foundation for a future accessibility broker.

## Gaps and deviations

- No a11yd package or implemented bexos.accessibility.semantics tree/action protocol matching the RFC was found. The source explicitly leaves semantic trees to a future broker.
- Dioxus value documents and visual scene commands are not an exported accessibility tree. Automatic semantic emission, redaction rules, and assistive action routing remain absent.
- Screen readers, braille/switch services, semantic traversal, and an a11yd migration adapter are not implemented. Existing rendering/input benchmarks do not validate accessibility behavior.

## Sources and validation

Implementation and contract evidence: [services/scened/src/controls.rs](../../../services/scened/src/controls.rs), [services/scened/src/controls_migration.rs](../../../services/scened/src/controls_migration.rs), [idl/bexos/ui/graphics.fidl](../../../idl/bexos/ui/graphics.fidl), [lib/flatland/src/accessibility.rs](../../../lib/flatland/src/accessibility.rs), [lib/dioxus_dom/src/lib.rs](../../../lib/dioxus_dom/src/lib.rs).

Relevant test sources and Bazel targets: [services/scened/tests/controls.rs](../../../services/scened/tests/controls.rs).

Detailed guides and previously recorded validation: [scened](../../scened.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
