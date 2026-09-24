# IDL And ABI

## FIDL Compiler

BexOS uses a local FIDL dialect implemented in `//tools/fidlc`. The compiler has parser, lexer, validator, capability splitting, IR, and Rust backend modules. Bazel invokes it through `bexos_fidl_rust` in `build/rules/fidl.bzl`.

Generated Rust bindings are build artifacts and are not checked in.

## Current FIDL Targets

`//idl` currently generates Rust libraries for:

- `camera_fidl_rust`
- `kernel_fidl_rust`
- `migration_fidl_rust`
- `bootstrap_fidl_rust`
- `app_debug_fidl_rust`
- `app_lifecycle_fidl_rust`
- `app_manager_fidl_rust`
- `app_opener_fidl_rust`
- `app_version_manager_fidl_rust`
- `app_worker_fidl_rust`
- `job_fidl_rust`
- `net_fidl_rust`
- `block_fidl_rust`
- `diskimage_fidl_rust`
- `archive_fidl_rust`
- `fs_fidl_rust`
- `vfs_manager_fidl_rust`
- `hardware_manager_fidl_rust`
- `i2c_spi_fidl_rust`
- `serial_fidl_rust`
- `ethernet_fidl_rust`
- `power_fidl_rust`
- `tracing_fidl_rust`
- `tee_manager_fidl_rust`

## Self runtime accounting

`TaskControl.GetRuntimeStats` is appended after the existing methods and accepts
no process or thread selector. It returns the calling thread's and process's
scheduled CPU nanoseconds. Process totals include exited worker contributions;
blocked time and synthetic fair-scheduling charges are excluded. The buffered
userspace wrapper uses fixed IPC storage. Scheduler snapshot `BEXSCH02` preserves
the accounting clock and retained thread totals. `BEXSCH01` remains readable and
reencodes unchanged until execution activates the new diagnostic epoch; historical
CPU time absent from that format is not invented.

The private `ShellControl.BeginMeasurement` method (ordinal 5) starts an identified
scened diagnostic epoch for workload IDs 1–5. It requires an idle presentation
pipeline and changes no scene, rendering policy, or scheduling deadlines. The
fixture receives the additional method through its prototxt capability grant.
Diagnostic windows restart after scened replacement; presentation state and
capability ownership continue to use the existing migration records.

## Kernel FIDL Surface

The kernel FIDL library is assembled from:

- `types.fidl`: universal rights, signals, status values, hardware access, and system power states.
- `object.fidl`: close, duplicate, physical VMO, legacy pin/unpin,
  IOMMU-domain create/map/unmap DMA, and memory stats.
- `ipc.fidl`: channel creation, asynchronous message read/write, synchronous
  call/read-call/reply-call ABI entries with absolute deadlines and reply-token
  handles, channel scheduling policy, and socket stream
  creation/read/write/shutdown/info.
- `memory.fidl`: VMO creation, mapping, VMARs, COW clone, unmap, and destroy operations.
- `scheduler.fidl`: fair/deadline profile structures and profile minting.
  `system.fidl` additionally carries v2 hierarchical resource-group limits,
  privileged group lookup, GPU reservation handles, and per-process realtime
  launch authorization.
- `task.fidl`: thread creation/exit, futexes, wait-many, profiles, affinity, yield,
  and self-only scheduled CPU statistics.
- `time.fidl`: monotonic/boot-time/realtime clocks and the read-only vDSO time
  page handle.
- `system.fidl`: privileged process/resource-group/thread/interrupt/checkpoint/power operations. `CreateProcess` returns the process handle, address-space handle, and a restricted root-VMAR construction handle for hierarchical setup.
- `debug.fidl`: kernel process listing and platform update status/control.
- `tracing.fidl`: privileged kernel trace producer attach/detach operations.
- `migration.fidl`: kernel-authorized process migration and handover operations.
- `restricted.fidl`: protocol-ID-14 restricted state binding, non-returning
  entry, unbind, and thread kick operations. The generated transport uses the
  fixed no-std state-page ABI in `//lib/restricted_abi`; it is a kernel
  execution primitive and does not implement Linux syscalls.

## App FIDL Surface

Current app-facing FIDL includes:

- `app/bootstrap.fidl`: startup resources, structured D1 driver resources,
  internal driver lifecycle handles, namespace paths, migration handles, service
  grants, config handles, linker data, and trace producer descriptors.
- `app/debug.fidl`: app process debug listing.
- `app/lifecycle.fidl`: list/install/uninstall/launch apps, begin migration, read migration status, and process progress.
- `app/manager.fidl`: web URL install, well-known reload, and domain association query surface.
- `app/migration.fidl`: two-party migratable service protocol and state receiver protocol.
- `app/opener.fidl`: app opener protocol for URLs, file MIME handlers, direct app opens, preferred interface binding, and user default handler selection.
- `app/version_manager.fidl`: list installed package versions, pin active
  versions, rollback to the previous pinned version, and prune inactive
  versions.
- `app/worker.fidl`: privileged appd-owned background worker launch surface used
  by `jobd`, including declaration lookup, package instance IDs, worker stop,
  and package-policy watcher callbacks.
- `job/scheduler.fidl`: `bexos.job.Scheduler`, `JobControl`, job specs,
  records, states including `LOCKED_USER`, monotonic/realtime/waiting-anchor
  timebase, run statuses, constraints, and network requirements.

## Storage And Filesystem FIDL

Current storage/filesystem contracts include:

- `storage/block.fidl`: block device info, buffer registration, FIFO handle, and migration markers.
- `storage/diskimage.fidl`: attach a file as a block device, or create/open a
  versioned encrypted image with UUID and virtual-size metadata.
- `storage/archive.fidl`: mount package or memory archives.
- `fs/fs.fidl`: node, file, directory, filesystem operations, raw managed BexFS
  format/mount, capacity reporting, transactional resize, and scoped unmount.
- `vfs/manager.fidl`: package store initialization with DiskImage and BexFS
  manager capabilities, package archive read/write/delete/list, system data
  directory lookup, user unlock with U-KEK VMO, user home/app-data directory
  lookup, and shared-vault directory lookup.

## Hardware And Power FIDL

Current hardware/power contracts include:

- `hardware/manager.fidl`: privileged `DeviceRegistry` for parented device-node
  registration/unregister, typed hardware resources, post-order removal
  initiation, and `DriverLifecycle.PrepareStop`. Its `BusType` enum includes
  I2C and SPI for statically routed peripheral grants.
- `hardware/i2c_spi.fidl`: scoped `I2cDevice` and `SpiDevice` protocols plus
  private deterministic fixture-control methods. Device transfer requests do not
  contain an address or chip-select field; those are fixed by topology and the
  granted channel.
- `power/power.fidl`: wake leases, provider-backed power snapshots,
  telemetry/performance provider protocols, system power requests, and power
  snapshot watcher callbacks.
- `net/net.fidl`: TCP/UDP/DNS/link-status APIs plus link watcher callbacks.
  The wire API remains unchanged while the implementation accepts dual-stack
  IPv4/IPv6 socket addresses and returns mixed A/AAAA resolver results.
- `time/time.fidl`: time quality, sync/manual-time controls, server
  configuration, `RtcHardware`, and time-quality watcher callbacks.
- `user/manager.fidl`: user create/update/delete/lock/unlock/list/get APIs plus
  user-state watcher callbacks used by lock-aware services.
- `kernel/system.fidl`: privileged system operations including clock/power
  control and forced process termination.
- `hardware/serial.fidl`: serial configuration, read/write, and interrupt-channel access.
- `hardware/camera.fidl`: sample permission-annotated camera controller.
- `hardware/ethernet.fidl`: raw Ethernet device info, buffer registration, FIFO channel access, and start/stop control.
- `power/power.fidl`: device power control, public power manager service, wake leases, and scheduler-readable power snapshots.
- `time/time.fidl`: public `TimeManager` status, force-sync, manual-time, and
  server-configuration surface for the storage-installed `timed` service.

## Network FIDL

`idl/bexos/net/net.fidl` defines the public netstack surface:

- `Netstack.ConnectTcp`, `ListenTcp`, `CreateUdpSocket`, `ResolveHost`, and
  `GetLinkStatus`;
- TCP socket and listener control protocols;
- UDP datagram control protocol;
- IPv4/IPv6 socket address types and bounded socket options. IPv6 rollout added
  only prototxt configuration fields and versioned internal migration records;
  it did not change the Netstack, socket, TimeManager, or TlsTrustManager FIDL
  wire ABIs.

The std libc compatibility layer calls kernel services through the generated
FIDL syscall transport. `clock_gettime` uses `Clock.GetTime`, TCP/UDP socket
setup uses `bexos.net.Netstack`, TCP byte streams use kernel `SocketControl`,
and pthread/futex compatibility entrypoints call `TaskControl`. `ProfileProvider`
and the scheduling-policy methods on `TaskControl` are routed through the same
guest syscall path, so userspace can mint profile handles, assign them, adjust
CPU affinity, yield, and wait on handle signals without a host-only shortcut.
The QEMU runtime stores schedulable thread contexts separately from process
address spaces, so same-process thread creation and futex block/wake both go
through the kernel FIDL route.

## Time FIDL

`idl/bexos/kernel/time.fidl` exposes monotonic, boottime, realtime reads, and
`Clock.GetVdsoTimePage`, which returns a read-only map-capable VMO containing
the versioned seqlock `TimePageV1` ABI. Realtime is derived from monotonic time
plus a kernel-held UTC offset and any active bounded slew.
`SystemPrivileged.AdjustClock` is gated by `SET_TIME`, only accepts realtime
adjustments, steps immediately with a zero slew rate, and accepts signed
nonzero slew rates only within +/-500 ppm and in the correction direction.

`idl/bexos/time/time.fidl` defines `bexos.time.TimeManager`, including
`GetTimeQuality`, `ForceSync`, `SetManualTime`, and `SetTimeServers`. The
current implementation accepts NTS enablement and reports
`ClockSource::NTS_SECURE` after NTS-KE, cookie, and authenticated NTP response
validation succeeds. The same
library defines `bexos.time.RtcHardware` with UTC-nanosecond `ReadUtc` and
`WriteUtc`.

## Tracing FIDL

`idl/bexos/tracing/trace.fidl` defines public
`bexos.tracing.TraceController` with `StartSession`, `StopSession`, and
`GetStatus`, plus system-privileged `bexos.tracing.TraceRegistry` with
producer register/replace/unregister. It also defines trace categories, buffer
modes, session states, and `TraceOutputFormat { PERFETTO,
LEGACY_BEXOS_FXT }`. The library imports `bexos.kernel` and uses
`bexos.kernel.Status` rather than duplicating a local tracing status enum.

The current generated target is `//idl:tracing_fidl_rust`. The debugd bridge is
also documented in `idl/bexos/debug/v1/debug_service.proto` and implemented on
the current `//lib/debug_wire` framed protocol.

## Protobuf Schemas

`idl/bexos/app/manifest.proto` defines app manifests:

- package name and display name;
- structured `bexos.version.SemVer` package version, multi-version policy, and
  minimum BexOS ABI metadata;
- process entries with runner, service flag, dependencies, wave, lifecycle, runner options, and resource group;
- process intent filters for opener schemes, HTTPS domains, MIME types, and provided interfaces;
- package/process structured permission declarations;
- exposed and consumed services;
- capability metadata;
- app manifest job declarations with target component, timing, persistence,
  execution budget, and power/network constraints;

`idl/bexos/domain/association.proto` defines `DomainPolicyRecord`, the protobuf
payload stored in `domain_associations.redb` for appd-owned well-known cache
entries. The redb key is the canonical domain and the value is manual protobuf
bytes.

`bexos.app.lifecycle` includes `RequestPermission` for optional app
permissions and privileged component-config `GetComponentConfig`,
`SetComponentConfig`, and `ResetComponentConfig` operations. `RequestPermission`
names the caller package/process/UID, the optional permission, requested values,
and the explicit `service_name`/`capability` selector for the provider handle to
mint. Appd validates that selector against the consumed-service declaration and
the provider capability's permission annotation, persists the grant, and returns
the live optional channel in `granted_handle`; if provider delivery fails after
persistence, the response is `LAUNCH_FAILED` with state `GRANTED`, the persisted
values, and no handle so retry can rebind. Config mutations use
expected-generation compare-and-swap fields and return a dedicated
`ComponentConfigStatus` so callers can distinguish not-found, invalid snapshot,
generation conflict, busy migration, rejected, timeout, and storage failures.
Requested and granted permission values are represented as bounded
`vector<string>` fields.
- resource groups;
- driver metadata and bind rules, including `BUS_I2C` and `BUS_SPI`;
- component config schema.

`idl/bexos/hardware/i2c_spi_topology.proto` defines the static topology schema
for I2C/SPI controller services. The source topology is prototxt and Bazel
compiles it into packaged protobuf configuration.

`idl/bexos/version/version.proto` defines structured package version metadata:
`SemVer`, `MultiVersionPolicy`, health state, and package-version records used
by app manifests, registry pins, and VersionManager responses.

`bexos.app.bootstrap.Startup` is emitted as ABI v9 for new launches. The message
carries explicit `NamespaceEntry` records separately from positional role
resources, retains `namespace_paths` for older receivers, keeps component config
and linker metadata in separate optional VMO fields, can carry a trace producer
descriptor VMO plus producer/PID/TID metadata, includes a bounded vector of
structured D1 `DriverResource` records plus an internal driver-lifecycle channel,
and now carries initial incoming lazy-service bindings with their descriptors,
lazy idle timeout, and lazy generation metadata. Generic `resources` remains
available for non-driver roles. Userspace decoding remains compatible with
startup ABI v2-v11. Decoders for v6-v10 are frozen; v11 appends retained
driver-host controller and recovery handles without changing older layouts.

`bexos.app.manifest.ExposedService` includes the current lazy-service manifest
ABI: `activation` defaults to `EAGER`, `idle_timeout_ms` is optional and defaults
to 2,000 ms for lazy providers, and `provider_process` is optional only when the
package has exactly one native service process. Lazy declarations are validated
against singleton or user-scoped singleton native providers with
`HEART_TRANSPLANT`; ambiguous, device-bound, multiple-instance, and conflicting
same-process declarations fail validation. `bexos.app.ServiceDirectory` is the
appd-hosted runtime connection surface, generated from FIDL by Bazel.

`idl/bexos/platform/assembly.proto` defines assembly input bundles, product definitions, component config overrides, and assembly manifests.

`idl/bexos/platform/config.proto` defines platform metadata, runner policy,
TEE policy, driver policy, update policy, and additive app lifecycle policy.
The lifecycle policy supplies backward-compatible defaults for probation
promotion, crash-window thresholding, and restart delay.

`bexos.vfs.manager.VfsManager` includes
`GetPackageArchiveAttributes` at ordinal 67. It returns logical archive size and
filesystem-allocated bytes for a versioned package key so appd can populate the
existing `VersionManager.disk_usage_bytes` and prune `reclaimed_bytes` fields
without changing the public app version-manager wire contract.

`idl/bexos/platform/device.proto` defines board/device records, hardware features, memory layout, boot configuration, CPU topology, BootFS manifests, and system-image manifests.

`idl/bexos/security/trust.fidl` defines the `trustd` app-signing trust surface.
`ValidateAppSigner` keeps ordinal 1 and now carries signature algorithm plus
required tier, returning granted tier, root anchor id, signer leaf fingerprint,
and accepted algorithm. Enterprise install and revocation update keep ordinals
2 and 3; enterprise-root removal and listing are new system-privileged ordinals
4 and 5. `TlsTrustManager.GetTlsRootBundle` remains wire-compatible.
`idl/bexos/security/trust.proto` defines app-signing root metadata and
revocation payloads used by the root-store and trustd mutation paths.

## Terminal and debug contracts

`//idl:tty_fidl_rust` defines provider-owned `bexos.tty.PtySession`: one-time
frontend stream acquisition, window size, terminal modes, signals, exit status,
and close. Successful stream acquisition returns three kernel socket handles;
error replies carry absent handles. `//idl:shell_fidl_rust` defines
`bexos.shell.ShellProvider.CreateSession`, which transfers the session server end
to the provider. Bindings are generated by Bazel from FIDL sources.

The privileged appd lifecycle channel accepts `BindDebugOpener` (ordinal 15) for
debugd; updated's lifecycle channel cannot select a shell identity. Kernel debug
process records include the resource-group name, parent ID, and main-thread identity.
Debugd resolves parent names from the same process snapshot when available.
BXD1 process fields 5–9 extend the prior response without changing its existing
fields. The actual trace encoding's output-format fields are now reflected in
the protobuf descriptor, correcting its stale producer/event field layout.

BXD1 shell methods use IDs 112–117 for open, exchange, resize, mode, signal, and
close. Streams exchange at most 16 KiB each per request/response. Guest stream
data uses kernel sockets; only the host bridge serializes it into BXD1. See
[CLI reference](cli.md) and [TTY design](rfcs/0033/README.md).
