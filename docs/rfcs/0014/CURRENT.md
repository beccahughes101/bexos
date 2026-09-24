# RFC 0014: Driver discovery and lifecycle — current implementation

- Reviewed: 2026-09-23
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0014](README.md)

## Implementation summary

Appd implements driver indexing, generic device topology, resource delegation,
process grouping policy and recovery for native D1 driver ELF processes. RFC 67
is the authoritative current architecture description.

## Implemented behavior

- Manifest driver metadata and bind rules produce ranked candidates. DeviceRegistry tracks canonical topological paths, arbitrary PCI/USB/I2C/SPI parentage and binding states; registration/unregistration are routed through the guest service.
- Appd retains canonical typed hardware resources and supplies rights-reduced duplicates. Per-device services retain identity, and teardown traverses descendants before parents.
- Recovery retains provider endpoints, retries/excludes candidates, and closes exhausted routes. Checkpoint state preserves topology, resource leases, host membership, pending acquisition, recovery metadata, and counters; drivers have separate migration adapters.

## Gaps and deviations

- The D2 WASM driver runtime remains a build smoke target, not a device-serving alternative.
- The earlier “IOMMU programming is future work” note is incomplete: current kernel/driver paths include domains and mappings. Physical SMMU correctness and cross-device DMA handover still need hardware validation.
- Network-backed acquisition is selected through pkgd's bounded prototxt
  hardware mappings. End-to-end guest acceptance remains incomplete; see RFC 55
  and RFC 67 current-state pages.

## Sources and validation

Implementation and contract evidence: [services/appd/src/driver_manager.rs](../../../services/appd/src/driver_manager.rs), [services/appd/src/device_registry.rs](../../../services/appd/src/device_registry.rs), [services/appd/src/hardware_resources.rs](../../../services/appd/src/hardware_resources.rs), [services/appd/src/recovery.rs](../../../services/appd/src/recovery.rs), [services/appd/src/checkpoint.rs](../../../services/appd/src/checkpoint.rs), [idl/bexos/hardware/manager.fidl](../../../idl/bexos/hardware/manager.fidl).

Relevant test sources and Bazel targets: [services/appd/tests/appd_tests.rs](../../../services/appd/tests/appd_tests.rs), [services/appd/tests/hardware_resource_tests.rs](../../../services/appd/tests/hardware_resource_tests.rs), [drivers/d2/testing/bexos/smoke/BUILD.bazel](../../../drivers/d2/testing/bexos/smoke/BUILD.bazel).

Detailed guides and previously recorded validation: [drivers storage](../../drivers-storage.md), [appd userspace](../../appd-userspace.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
