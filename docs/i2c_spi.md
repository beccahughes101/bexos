# I2C And SPI Services

The current tree implements separate D1 services for statically routed I2C and
SPI peripheral access:

- `bexos.service.i2cd`, built from `//services/i2cd:i2cd`
- `bexos.service.spid`, built from `//services/spid:spid`

Both services are user-space bus controllers. They expose scoped peripheral
channels to matched drivers through appd's device registry and do not expose raw
controller management to ordinary peripheral clients. The long-term hardware
architecture remains in [the design document](rfcs/0053/README.md); this page
records the behavior implemented in this repository today.

## Implemented surface

`idl/bexos/hardware/i2c_spi.fidl` defines `I2cDevice` and `SpiDevice`. A device
channel is bound to one configured peripheral, so transfer requests cannot name a
different I2C address or SPI chip select. `GetInfo` reports the bound address or
chip-select/configuration limits for diagnostics and driver setup.

I2C supports 7-bit and 10-bit target addresses. A transfer is an atomic bundle of
write and read operations. The controller emits repeated starts between bundle
operations and a final STOP. Failed transfers return explicit status values for
invalid requests, NACK, arbitration loss, timeout, queue overflow, oversize
bundles, expired locks and deterministic injected failures. The controller does
not replay a write that may have reached the backend.

SPI supports write, read and full-duplex operations in an atomic bundle. Chip
select is held for the whole bundle and released once the bundle completes or
aborts. Each peripheral has topology-defined mode and speed limits. The service
validates requested mode and speed before queueing work and applies the active
configuration only between bundles.

Both controllers use the shared `//lib/i2c_spi` arbitration and migration library.
Per controller limits are:

- 16 operations per bundle
- 4 KiB total transfer data per bundle
- 64 pending bundles per controller
- one second maximum request deadline
- 100 ms maximum `LockBus` lease

`LockBus` returns an ephemeral channel for the same peripheral. Transfers sent
through that channel hold exclusive bus ownership until the lease is released,
the peer closes, or the absolute expiry is reached. Other queued clients remain
in FIFO order and can time out while waiting behind the lease. Once a lease has
expired, that locked channel cannot reacquire ownership.

## Topology and registration

`idl/bexos/hardware/i2c_spi_topology.proto` describes static controller and
peripheral topology. The packaged fixture topology is authored as prototxt in
`services/i2cd/package/topology.prototxt` and compiled by Bazel into
`config/i2c_spi_topology.pb`; generated protobuf bytes are build artifacts and
are not committed.

The parser validates stable nonzero node IDs, unique controller/peripheral IDs,
matching bus-specific peripheral records, I2C address uniqueness per controller,
SPI chip-select uniqueness per controller, nonempty mode masks, and speed ranges.
The current fixture includes one deterministic I2C controller with a 7-bit sensor
and a 10-bit EEPROM, plus one deterministic SPI controller with two chip-selects.

Appd's manifest and hardware-registry bus enums now include I2C and SPI while
preserving existing enum values. Appd exposes separate private descendant
registrar capabilities for I2C and SPI. The services register configured
controller/peripheral nodes through those registrar channels, and each peripheral
receives only a scoped `BUS_CONTROL` channel.

The deterministic topology and backend are packaged with `i2cd` and `spid` for
service and migration validation. They are not wired into the normal QEMU product
as synthetic physical devices.

## Backend and fixture controls

`//lib/i2c_spi` defines a backend trait with separate start, complete and abort
operations. The deterministic backend models I2C register bytes, SPI exchanges,
delayed completion, injected failures, operation counters and bus-boundary
events. Fixture-control FIDL is exposed by the service manifests as a private
capability and retained across migration; ordinary peripheral clients do not get
that control channel.

This backend validates service semantics and replacement continuity. It does not
claim physical controller timing, DMA behavior, interrupt latency, DT/ACPI
discovery, GPIO integration, or hardware support.

## Heart transplant

Both services opt into `HEART_TRANSPLANT` in their prototxt manifests and provide
separately linked replacement archives:

- `//services/i2cd:replacement_archive`
- `//services/spid:replacement_archive`

Migration records preserve configured topology, endpoint identity, registered
node state, queued transfers, pending replies, deterministic backend contents,
SPI configuration, fixture clients and lock leases with absolute deadlines. While
bulk records are copied, normal request processing continues. During drain the
source stops starting new work, then finishes or aborts the bounded active
transfer before quiescence. Adoption skips cold topology registration so the
replacement keeps the transferred endpoints and registry state.

Malformed or incompatible records are rejected before adoption. Source-side
state remains usable after failed validation because completed operations are not
replayed into the replacement.

## Validation status

Focused host validation covers topology rejection, address and chip-select
isolation, I2C repeated-start/STOP boundaries, SPI CS boundaries, FIFO bounds,
request deadlines, bundle size limits, lock expiry and disconnect, SPI
configuration isolation, injected failures, migration of queued/active/completed
state, backend mutations, lease deadlines and malformed migration records. Appd
coverage checks I2C/SPI bus enum decoding and matching for scoped grants.

The following commands passed in the current validation pass:

```sh
bazel test //lib/i2c_spi:tests \
  //services/i2cd:runtime_tests \
  //services/spid:runtime_tests \
  //services/appd:appd_tests

bazel test //:heart_transplant_coverage_test

bazel build --config=aarch64 \
  //services/i2cd:i2cd \
  //services/i2cd:replacement_archive \
  //services/spid:spid \
  //services/spid:replacement_archive

bazel build --config=x86_64 \
  //services/i2cd:i2cd \
  //services/i2cd:replacement_archive \
  //services/spid:spid \
  //services/spid:replacement_archive

bazel build --config=aarch64 //services/appd:appd //services/appd:replacement_archive
bazel build --config=x86_64 //services/appd:appd //services/appd:replacement_archive

bazel run @rules_rust//:rustfmt
```

An AArch64 QEMU fixture that drives concurrent peripheral clients through live
service replacement has not yet been added to the maintained e2e matrix in this
working tree. The package builds above establish that the services and
replacement archives build for both guest architectures; they are not runtime
QEMU acceptance.

## Future work

Physical controller drivers, DT/ACPI topology discovery, GPIO service
integration, real DMA, real-time scheduling guarantees, I3C, and hardware timing
validation remain future work.
