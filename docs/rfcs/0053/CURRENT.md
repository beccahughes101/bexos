# RFC 0053: I2C and SPI services — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0053](README.md)

## Implementation summary

Separate I2C and SPI services implement scoped peripheral transactions against a deterministic backend. They are not physical controller drivers.

## Implemented behavior

- Prototxt topology specifies controller/peripheral identities, I2C addresses, and SPI chip-select/mode/speed bounds. Appd registration gives clients scoped BUS_CONTROL channels.
- The shared library implements bounded atomic bundles, FIFO arbitration, deadlines, lock leases, injected failures, and backend boundary events. Requests cannot select another peripheral’s address.
- Migration retains topology, queued/completed work, replies, backend contents, configuration, and absolute lock deadlines. Services drain/abort bounded active work and skip cold registration on adoption.

## Gaps and deviations

- The deterministic controllers are validation fixtures and are not normal QEMU synthetic devices.
- No physical MMIO/DMA controller backend, DT/ACPI discovery, GPIO integration, I3C, or measured realtime bus guarantee is supplied.
- Host tests and both-architecture package builds are recorded, but a maintained concurrent-client QEMU transplant fixture and physical timing validation remain absent.

## Sources and validation

Implementation and contract evidence: [lib/i2c_spi/src](../../../lib/i2c_spi/src), [services/i2cd/src](../../../services/i2cd/src), [services/spid/src](../../../services/spid/src), [services/i2cd/package/topology.prototxt](../../../services/i2cd/package/topology.prototxt), [idl/bexos/hardware/i2c_spi.fidl](../../../idl/bexos/hardware/i2c_spi.fidl), [idl/bexos/hardware/i2c_spi_topology.proto](../../../idl/bexos/hardware/i2c_spi_topology.proto).

Relevant test sources and Bazel targets: [lib/i2c_spi/BUILD.bazel](../../../lib/i2c_spi/BUILD.bazel), [services/i2cd/BUILD.bazel](../../../services/i2cd/BUILD.bazel), [services/spid/BUILD.bazel](../../../services/spid/BUILD.bazel).

Detailed guides and previously recorded validation: [i2c_spi](../../i2c_spi.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
