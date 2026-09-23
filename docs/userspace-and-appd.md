# Userspace And Appd

BexOS builds a standalone std-linked AArch64 `appd` ELF at
`//services/appd:appd_elf` through the BexOS libc shim. The external BootFS packages it at
`/boot/pkg/bexos.platform.appd/bin/appd`; the kernel loads that ELF into
an isolated EL0 address space and passes a bootstrap channel containing the
BootFS VMO and initial capabilities.

## Userspace Handoff

The kernel copies ELF segments, zeros BSS, creates a stack, switches to the
process page table, and transfers the bootstrap channel before `eret`. Services
preserve the same `_start(channel)` ABI while linking Rust `std` through
`//lib/bexos_libc`; D1 child drivers continue to use the no-std freestanding
startup, allocator, syscall transport, and linker support. The scheduler core is
SMP-aware, but the bare-metal EL0 runtime still launches the initial appd
process on CPU0 for this QEMU milestone.

## Manifest Build Flow

App manifests are protobuf text fixtures compiled at build time:

```sh
BAZELISK_HOME=/private/tmp/bexos-bazelisk-cache bazel build //services/appd:camera_provider_manifest_bin
BAZELISK_HOME=/private/tmp/bexos-bazelisk-cache bazel build //services/appd:camera_client_manifest_bin
```

The generated `.bexmanifest` and `.bootpkg` files are Bazel outputs and should
not be committed. Runtime appd code consumes protobuf wire bytes and
decodes them into owned Rust manifest structs.

Manifests may also declare scalar structured config schemas:

```proto
config_schema {
  fields {
    name: "channel"
    type: CONFIG_STRING
    required: true
    max_size: 16
    default_value { string_value: "stable" }
  }
}
```

Product assembly validates defaults and overrides, then packages the resolved
blob at `/pkg/config/component.bexconfig`. On launch, `appd` sends the
blob as `Startup.config` with `Startup.config_len`; older packages receive no
config handle and continue to launch normally.

Processes declare their runner with `Process.runner` and may attach typed
runner configuration through `google.protobuf.Any`:

```proto
processes {
  name: "camera_service"
  runner: "elf"
  service: true
  wave: 1
  runner_options {
    type_url: "type.googleapis.com/bexos.app.ELFRunnerOptions"
    value: "\n\027/pkg/bin/camera_service"
  }
}
```

`ELFRunnerOptions.path` identifies the executable inside the package namespace.
The appd decoder recognizes the ELF options type URL and preserves
unknown runner options for future runners.

`Process.wave` is optional. When present, appd treats the process as part of
automatic startup and launches lower-numbered waves before higher-numbered
waves. Wave `0` is valid. When the field is omitted, the process is
manual-only: appd does not start it during automatic boot, but the same runner
policy and launch path can start it later through an explicit manual request.
Appd advances from one wave to the next only after every launched process in
the current wave reports ready.

On the QEMU board, `debugd` is packaged as `bexos.driver.debugd` and starts in
wave `3`, after the early storage stack. It owns the QEMU debug socket v1
transport and reports readiness before appd releases BootFS.
`powerd` is packaged as `bexos.service.powerd` and starts in wave
`4`, after the core D1 drivers and storage services report ready. App-service
hands it dedicated `DevicePowerControl` endpoints for the PCI root and NVMe
driver manager channels, then publishes its singleton
`bexos.power.PowerManager` service.
After storage is initialized, appd hands `debugd` an app lifecycle
client channel. `debugd` stays transport-only for app commands: it proxies list,
install, uninstall, and launch requests to appd, while appd owns
registry mutation, policy, and lifecycle state.

## Appd Broker

`services/appd` owns the host-testable broker logic:

- `PublishInterface` registers provider package, exposed service metadata, and
  endpoint capability.
- `GetInterfaces` returns authorized matches filtered by protocol and metadata.
- `GetSingletonInterface` returns authorized global singleton services.
- `GetUserScopedSingleton` requires a user context before returning a
  user-scoped singleton.

The broker filters capability groups at bind time using generated FIDL
permission metadata. For launched consumers, appd mints one channel pair
per authorized capability group, passes client endpoints in the startup grant
set, and notifies the provider manager channel with the provider endpoint and
method ordinals. Once these endpoints are handed out, steady-state messages are
expected to remain peer-to-peer.

`powerd` uses the same channel handoff convention for driver power
control. It tracks wake-lease handles and denies suspend-to-RAM while leases
are active. For system transitions it first asks registered D1 drivers to enter
`D3_OFF`, then calls `SystemPrivileged.RequestSystemPowerState`; if suspend
returns or fails, it asks drivers to restore `D0_FULL_POWER`. On the maintained
QEMU AArch64 product the kernel side uses QEMU PSCI through SMC for CPU standby,
reset, and shutdown; Trusty runs as the secure-world payload on the real QEMU
product, while the emulated product selects the software TEE driver explicitly.

## Kernel Service Pass-Through

The kernel control protocols live in `idl/bexos/kernel` and build through
`//idl:kernel_fidl_rust`. App-service publishes `ChannelControl`,
`VirtualMemory`, and `TaskControl` as public singleton interfaces backed by
kernel endpoint capabilities. `SystemPrivileged` is published as a singleton
whose generated method capabilities carry the privilege boundary. Process and
power-management operations remain gated by `BEXOS_SYSTEM_PRIVILEGED`; realtime
clock adjustment is exposed through the narrower `SET_TIME` capability.
The privileged surface includes `RequestSystemPowerState`, which is used by
`powerd` after userspace policy and D1 quiescing have completed, and
`AdjustClock`, which is used by `timed` after SNTP sync.

App-service also has a host-testable lifecycle debug model in
`services/appd::debug`, backed by `//idl:app_debug_fidl_rust`. The live
QEMU v1 external debug endpoint is `debugd`; the appd model is the
internal surface that later debugd/appd integration can query for richer
launch and lifecycle state.

## App Registry And Lifecycle

`//lib/redb` wraps upstream redb behind a BexOS `BlockStore` trait with
`len`, `read_at`, `write_at`, `set_len`, `sync`, and `close`. The wrapper owns
the custom storage backend integration so future appd and per-user
registries can reuse redb without duplicating std/no-std feature decisions.

`//lib/app_registry` stores app package id, display name, manifest cache bytes,
archive content root, signer key id, install source, lifecycle state, protected
status, and archive path. It has an in-memory guest registry used by the current
QEMU appd path plus a redb-backed persistent registry target for host and
future block-backed registry storage. The current system registry is global;
per-user encrypted registries in home storage remain a documented extension
point, not part of this milestone.

`//lib/permission_store` stores system permission declarations and per-user
grant records. It has an in-memory no-std implementation used by the current
guest appd path and a `std + redb_backend` host implementation following
the same `BlockStore` pattern as `//lib/app_registry`. App-service registers
package declarations at boot/install, auto-grants required package/process
permissions at launch, and removes grant records on uninstall. Final guest
mounting at `/system/state/permissions.redb` and
`/vault/user_<uid>/permissions.redb` remains integration work.

The install unit is a signed `.bex` archive containing
`package.bexmanifest`. Debug uploads send signed bundle bytes to `debugd`;
`debugd` forwards the bundle to appd through
`bexos.app.AppLifecycleControl.InstallBundle` using a VMO handle. App-service
verifies the signature against trusted package keys, imports the manifest into
the registry, and asks vfsd to store the archive under `STORAGE/pkg`.
Uninstall removes mutable packages from the registry, vfsd package store, and
permission store; protected boot/system packages are denied.

QEMU still imports legacy BootFS `.bexmanifest` entries as protected manifest
cache records while boot services are converted to signed boot bundles. The
device system image now declares autoinstall packages in prototxt, and the
QEMU storage verifier is seeded as a signed `.bex` archive under the package
store. Decoding the system image autoinstall list inside appd is future
work.

The kernel now exposes explicit process-management hooks for appd:

- `VirtualMemory.MapInVmSpace` and `VirtualMemory.UnmapInVmSpace` map VMO
  ranges into a specified VM-space handle.
- `SystemPrivileged.StartThreadInProcess` starts a thread in a specified
  process and VM-space pair.

See [Kernel FIDL Services](kernel-fidl-services.md) for the service contracts,
SVC ABI, and current runtime limits. Kernel control-plane records, IPC payload
storage, and FIDL read-message staging are allocation-backed; the boot loader
remains a linked-stub handoff until package loading and live page-table
installation land.

## Runner Architecture

`services/appd::runner` owns the host-testable launch path. A
`RunnerRegistry` parses `Process.runner`, enforces runner policy, dispatches to
the concrete runner, and returns the launched process/thread handles plus the
appd side of the initial service-manager channel.

The current policy allows ELF only for `PlatformCore` and `SystemHardware`
trust tiers. Standard consumer packages that declare `runner: "elf"` are
rejected before appd resolves executable bytes or asks the kernel to
create a process. Signature-chain validation is still package-install work; the
runner layer receives the already-classified package trust tier.

`PackageImageResolver` is the package/storage boundary for executable images.
For BootFS packages it reads immutable boot entries directly. For disk packages,
appd asks `vfsd.GetPackageDirectory` for a read-only package root. vfsd
opens `pkg/<package>.bex` from its managed STORAGE package store, asks
ArchiveFS to verify the Ed25519 signature and BLAKE3 chunk hashes, then returns
the mounted archive root as the process `/pkg` namespace. For ELF it resolves
`ELFRunnerOptions.path` to:

- readable ELF bytes for appd validation;
- a file-backed executable VMO handle that appd maps into the child VM
  space.

The runner does not write executable bytes into anonymous VMOs. Disk-launched
ELFs come from the signed ArchiveFS package root; the QEMU verifier package is
installed as `pkg/bexos.platform.storage_verify.bex`, imported into the app
registry as a protected system package, and marked running after appd
launches it.

The ELF parser and load planner live in `kernel/core::loader` as a
`no_std + alloc` shared API. App-service delegates ELF validation to that
library, and the same `LoadPlan` type is available to the kernel bootstrap path.
`kernel/core::loader::EmbeddedPackageProvider` implements the first immutable
package provider behind the replaceable package-provider trait; persistent
virtio/block/filesystem package storage remains a future provider.

## Permissions

App manifests now use structured permission declarations rather than legacy
string permissions. Each declaration has a name, optional scoped values,
required/optional requirement, and usage description. The current launch path
auto-grants required permissions because trusted user consent UI is not wired
yet.

Optional grants are requested through
`bexos.app.lifecycle.AppLifecycleControl.RequestPermission`. Requests are
accepted only for `PERMISSION_OPTIONAL` declarations and only for values that
are a subset of the manifest declaration. Granted values are propagated in
startup service grants and provider binding notifications as:

```text
service|protocol|capability|ordinal,ordinal|value,value
```

## ELF Runner

The ELF runner accepts ELF64 little-endian AArch64 `ET_EXEC` and `ET_DYN`
images. It validates bounded program headers, loadable segment bounds,
entrypoint placement inside an executable segment, and rejects WRITE+EXECUTE
segments. `ET_DYN` images use a fixed initial load bias until address-space
layout selection exists.

Launch sequence:

1. Resolve and validate the package executable.
2. Create the process and address space through `SystemPrivileged.CreateProcess`.
3. Map each `PT_LOAD` segment from the package-provided file VMO with the
   segment's requested read/write/execute rights.
4. Represent page-aligned BSS with anonymous zero VMOs; reject partial-page BSS
   layouts that cannot be represented safely.
5. Create and map a read/write stack VMO.
6. Create the initial service-manager channel.
7. Start the main thread with `SystemPrivileged.StartThreadInProcess`, passing
   the child channel endpoint as `arg_handle`.

The generated FIDL bindings currently represent `handle:OPTIONAL` as a raw
handle value; raw handle `0` is the no-handle convention.

## Locale descriptor

Source integration is present; the local implementation and acceptance plan
remain incomplete. See [RFC 0066 current gaps](rfcs/0066/CURRENT.md#current-gaps-in-the-approved-local-scope).

The current envelope is version 10. Its optional locale descriptor carries a
read-only CLDR VMO, length, data generation, and encoded locale snapshot. Legacy
startup versions remain decodable. Appd supplies locale state after localed is
ready; early boot services do not wait on localed. Native applications transfer
the descriptor into `bexos_i18n_client::Client` or its context adapter, whose
mapping lifetime covers the borrowed ICU provider. See [localization](localization.md).
