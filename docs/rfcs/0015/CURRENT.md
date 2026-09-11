# RFC 0015: Assembly and component configuration — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0015](README.md)

## Implementation summary

Typed assembly configuration, persistent user preferences, policy locks, and receiver-coordinated live updates are implemented.

## Implemented behavior

- Bazel compiles prototxt schemas, assembly overrides, and policy into package artifacts. The shared component-config library validates types, constraints, fingerprints, and BEXCFG snapshots.
- Appd supplies startup configuration and registers package schema/version identity with prefsd. Effective values combine defaults, operator assignments, and permitted user overlays; explicit product/operator locks control precedence.
- Prefsd stores nonzero-UID preferences inside encrypted homes, uses committed alternating slots, and coordinates prepare/commit/abort with live receivers. CLI config/prefs operations use generation checks.
- Appd and prefsd migration preserve revision/transaction state and coordinate frozen updates; older instances without a live receiver retain their launch snapshot.

## Gaps and deviations

- The older overview’s claim that all live reconfiguration is future work is stale. Live updates apply only to registered receivers and declared supported fields.
- A general Settings application and enterprise MDM client are not supplied by the typed management API.
- Persistence and migration tests do not establish every failure/reboot interleaving or hardware durability property; indeterminate commit decisions require the existing retry protocol.

## Sources and validation

Implementation and contract evidence: [lib/component_config/src](../../../lib/component_config/src), [services/prefsd/src](../../../services/prefsd/src), [services/appd/src/guest/preferences.rs](../../../services/appd/src/guest/preferences.rs), [tools/config_compiler](../../../tools/config_compiler), [idl/bexos/preferences/preferences.fidl](../../../idl/bexos/preferences/preferences.fidl), [idl/bexos/app/manifest.proto](../../../idl/bexos/app/manifest.proto).

Relevant test sources and Bazel targets: [lib/component_config/tests/config_tests.rs](../../../lib/component_config/tests/config_tests.rs), [services/prefsd/tests/prefs_tests.rs](../../../services/prefsd/tests/prefs_tests.rs), [tools/config_compiler/tests](../../../tools/config_compiler/tests).

Detailed guides and previously recorded validation: [services](../../services.md), [appd userspace](../../appd-userspace.md), [cli](../../cli.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
