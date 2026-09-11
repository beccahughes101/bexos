# RFC 0055: On-demand driver provisioning — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0055](README.md)

## Implementation summary

Local driver matching and signed package verification exist; automatic registry-backed provisioning for unmatched hardware does not.

## Implemented behavior

- Appd ranks installed manifest bind rules against device properties, applies hardware policy, and manages failed binding/recovery with retained resources.
- The package/update libraries can verify archives and TUF targets. Native driver packages use ordinary lifecycle and heart transplant mechanisms.

## Gaps and deviations

- No canonical hardware-query serializer, trusted driver-registry lookup client, specificity-fallback network search, or automatic install-and-rebind pipeline matching the RFC was found.
- Public appd URL fetching is unavailable and updated’s feed is seeded, so those interfaces do not supply online driver discovery.
- The TOML configuration example is a retained design sketch; current app/platform configuration uses prototxt and has no deployed driver_sources.toml parser.
- Firmware dependency resolution, offline registry policy, provisioning consent, and end-to-end new-device activation remain implementation work.

## Sources and validation

Implementation and contract evidence: [services/appd/src/driver_manager.rs](../../../services/appd/src/driver_manager.rs), [services/appd/src/device_registry.rs](../../../services/appd/src/device_registry.rs), [services/appd/src/recovery.rs](../../../services/appd/src/recovery.rs), [services/appd/src/manager.rs](../../../services/appd/src/manager.rs), [lib/tuf/src/lib.rs](../../../lib/tuf/src/lib.rs), [idl/bexos/app/manifest.proto](../../../idl/bexos/app/manifest.proto).

No dedicated implementation test for this RFC was found in the reviewed tree.

Detailed guides and previously recorded validation: [drivers storage](../../drivers-storage.md), [appd userspace](../../appd-userspace.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
