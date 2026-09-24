# RFC 0055: On-demand driver provisioning — current implementation

- Reviewed: 2026-09-23
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0055](README.md)

## Implementation summary

Local driver matching, signed package verification and the automatic
registry-backed acquisition path for unmatched PCI hardware are implemented.

## Implemented behavior

- Appd ranks installed manifest bind rules against device properties, applies hardware policy, and manages failed binding/recovery with retained resources.
- Pkgd prototxt mappings carry bounded PCI selectors and priority. Appd selects
  exact BDF, then vendor/device, then class fallback; queues the request without
  blocking the registrar; verifies the downloaded manifest against the waiting
  identity; installs durably; and re-enters normal binding.
- Pending node identities, resolver channels and retry state are checkpointed.
  Duplicate requests are bounded by node ID and the configured request limits.

## Gaps and deviations

- The path uses pkgd's signed OCI/TUF resolver rather than public appd URL
  fetching. Default products intentionally configure no production trust root or
  mapping, so an unmatched device remains unbound with local diagnostics.
- The TOML configuration example is a retained design sketch; current app/platform configuration uses prototxt and has no deployed driver_sources.toml parser.
- End-to-end dual-architecture guest acceptance and physical-device activation
  remain unverified. Firmware dependency activation and user-facing provisioning
  consent remain implementation work.

## Sources and validation

Implementation and contract evidence: [services/appd/src/driver_manager.rs](../../../services/appd/src/driver_manager.rs), [services/appd/src/device_registry.rs](../../../services/appd/src/device_registry.rs), [services/appd/src/recovery.rs](../../../services/appd/src/recovery.rs), [services/appd/src/manager.rs](../../../services/appd/src/manager.rs), [lib/tuf/src/lib.rs](../../../lib/tuf/src/lib.rs), [idl/bexos/app/manifest.proto](../../../idl/bexos/app/manifest.proto).

No dedicated implementation test for this RFC was found in the reviewed tree.

Detailed guides and previously recorded validation: [drivers storage](../../drivers-storage.md), [appd userspace](../../appd-userspace.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
