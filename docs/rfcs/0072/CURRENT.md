# RFC 0072 current implementation

- Reviewed: 2026-09-26
- Design: [RFC 0072](README.md)
- Status: implemented; host/build validation complete, live QEMU execution was
  intentionally not run for this change

## Implemented architecture

`bexos.component.runner` is defined in
`idl/bexos/component/runner.fidl`. `ComponentRunner.Start` receives the resolved
URL, opaque typed program metadata, a package directory, resolved dependency
directories and metadata, the existing versioned bootstrap `Startup` channel,
and an attenuated component-job handle. Public component starts are
directory-capability only; immutable VMO fallback exists solely on the private
`NativeRunnerHost.Prepare` bootstrap transport for BootFS before the directory
service is available. `ComponentController` supplies graceful stop, kill,
signal, and typed late-bound service forwarding. The FIDL compiler does not yet
support event syntax, so `ComponentRunnerEvents.OnReady` and `OnStop` are
callbacks. Shared helpers enforce at-most-one ready and terminal notification;
appd treats malformed ordering or duplicate terminal messages as a protocol
failure and terminates the component job.

Every component launch has its own immutable one-process kernel job and initial
process. Job policy binds package identity, resource group, hardware tier,
realtime permission, and process limit. Appd retains administrative handles.
The fixed BootFS `bexos.platform.native_runner` receives only delegated job
management, process start/inspection, address-space mapping, transfer, and read
rights. The kernel denies process creation through delegated handles and
enforces the one-process limit even for the privileged creation API. Job state,
membership, and handles participate in full and incremental snapshots, and job
termination recursively exits every member.

Appd's only remaining ELF mapper is the small bootstrap loader in
`services/appd/src/runner/bootstrap.rs`. It accepts only the retained BootFS
native-runner image and rejects `PT_DYNAMIC`, TLS, malformed program headers,
and all other targets. The complete ELF mapper, dynamic dependency graph,
relocations, TLS/linker metadata, VMAR layout, and process-start path live in
`lib/native_loader` and are used by `services/native_runner`. At boot appd copies
the fixed image from BootFS into a private retained VMO; it does not resolve
that image from the mutable package registry. The retained image handle is
included in appd migration state.

For each launch appd starts one disposable native host in a separate host job,
creates the target job/process, and calls `NativeRunnerHost.Prepare`. The host
maps either the native target or a statically linked registered provider into
the pre-created target process and returns its main thread through
`OnPrepared`. Appd sends the normal `ComponentRunner.Start` with a fresh
bootstrap endpoint, completes trace/configuration construction, and sends the
versioned `Startup` resources before the provider can proceed to readiness.
Direct ELF uses the host as the standard runner
endpoint; WASM/Starnix receive the endpoint directly in their target process.
Neither the host nor a provider can create a second target process or change
job policy. For component-runner providers the host also rejects `PT_DYNAMIC`
and declared native dependencies before mapping the provider.

Package-backed launches, including migration candidates, pass read-only package
and dependency directories. Registered providers are opened through their
independently authenticated package directories on the normal filesystem path;
the native host's bounded read-only adapter consumes immutable VMO snapshots
only during early BootFS, before a directory service exists. The
native host and WASM/Starnix providers validate canonical `/pkg/...` paths,
formats, type URLs, size limits, and runner-specific options before reading.
The private native-host preparation contract can use bounded immutable VMO
snapshots for early BootFS when a directory service is unavailable. Runners
reconstruct `/pkg` and `/deps` namespace entries from the public directory
capabilities. Appd owns package and dependency selection but forwards the
manifest's typed runner options opaquely; WASM, Starnix, and direct ELF each
decode and validate their own type URL, options, path, format, and limits.

## Registry, packages, and updates

`RunnerPolicy` contains the runner-provider registry. Maintained AArch64 and
x86_64 product prototxts register `elf` as `DIRECT_ELF` and `wasm`/`nix` as
`COMPONENT_RUNNER` providers with package identity, executable path, and
expected signer. Validation rejects empty or duplicate normalized names,
traversal and non-`/pkg` paths, invalid field combinations, missing identities,
and signer/package mismatches. There are no implicit provider registrations in
code: an absent registry and unknown runners fail closed. Existing manifest
runner strings and runner-option protobuf encodings remain compatible.

The independently signed WASM and Starnix runner archives are ordinary
system-image packages. Appd has no compile-time runner ELF or digest and no
runner-kind execution switch: every `COMPONENT_RUNNER` registration follows the
same data-driven launch path. Component-owned
`.bexos/wasm_runner.bex` overrides are rejected during package validation; the
old nested update fixture has been replaced by an independently packaged
provider update.

Installing a registered provider version queues a provider rollout. Appd finds
every running heart-transplant consumer of that provider and reuses the normal
transaction to prepare it against the newly active provider. Consumers commit
independently while retaining service connections and migratable state. A
failed candidate rolls the provider active pin back to the previous version.
Restart-only consumers keep their already loaded provider and observe the new
version on their next launch. After all migratable consumers commit, appd clears
the provider rollback target.

## Lifecycle and failure boundary

Launch state records the component job, process/address space/thread,
controller/events channels, disposable host job/process/address space/thread,
provider identity/path/signer/version, and existing migration state.
Construction failures close all partial handles and terminate both jobs. Host
death, provider death, runner event-channel loss, controller peer closure, and
protocol violations terminate the affected component job, revoke its grants
during normal launch cleanup, and feed the existing restart/watchdog policy
without affecting appd or unrelated components.

`Stop` is forwarded to the component controller and establishes a five-second
deadline; expiration terminates both component and host jobs. `Kill` terminates
immediately. The shared userspace bootstrap runtime defines the versioned
`bexos.bootstrap.stop.v1` message and a cancellation hook. WASM cancels at its
executor loop, Starnix translates stop/kill/signal into its signal machinery,
and direct native binaries receive the versioned message. Native binaries that
do not implement the hook fall back to deadline termination. Successful starts
produce one readiness callback and one terminal callback; failure before
readiness produces only the terminal callback.

## Runner behavior retained

`wasm_runner` and `starnix_runner` serve `ComponentRunner` directly. WASM keeps
WASI Preview 2, Pulley execution, component-model dependency composition,
package-scoped caches, bounded resources, checkpoints, and the Dioxus renderer
profile. Starnix keeps its static single-task restricted-mode implementation,
filesystem and signal state, checkpoints, and translates
`ComponentController.SendSignal` without a private runner-control message. Each
component still receives its own provider process and job; caches never merge
failure domains.

## Validation evidence

The source includes unit/negative coverage for registry normalization and
signer/path combinations, nested-runner rejection, generated FIDL encode/decode
bounds, typed late-service forwarding, callback ordering and lifecycle
deduplication, peer-loss cleanup, five-second stop escalation,
launch-construction cleanup, job-right attenuation and escalation denial,
process limits/recursive termination/snapshot state, native ELF
dependencies/TLS, WASM/Dioxus, Starnix signals, normal and BootFS directory
adapters, provider-version state, and provider
replacement/rollback/restart-only selection paths. Generated FIDL/protobuf
Rust remains Bazel output and is not committed.

The combined `//testing/e2e/qemu/runners:rfc72_isolation_test_{aarch64,x86_64}`
targets launch native, WASM/Dioxus, and Starnix profiles together, inject a
fault into each runner type, verify appd and an unrelated Dioxus instance remain
live, and install an independently packaged WASM-provider replacement while a
client remains connected. The targets are built as part of validation but were
not executed, per the explicit instruction not to run QEMU. Exact commands and
results are recorded in [testing status](../../testing-status.md).

## Future expansion

The initial job policy intentionally permits exactly one component process.
Future multi-process component jobs may add child-process delegation and policy
without changing the public runner FIDL because the job capability is already
the lifecycle unit. Broader multi-task/container Starnix behavior remains a
future RFC 70 phase rather than an RFC 72 implementation gap.
