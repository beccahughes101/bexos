# Appd And Userspace

Appd supervises the selected system and per-user shells. See
[System and User UI](sysui.md) for configuration, session authority, and
recovery.

## Build Targets

`//services/appd` provides:

- `:appd`: std service library.
- `:appd_elf`: std-linked userspace ELF binary.
- `:appd_tests`: host tests.
- `:package_manifest`: compiled app manifest.
- `:replacement_elf` and `:replacement_archive`: update/migration artifact targets.

The manifest package ID is `bexos.platform.appd`, and its process uses the ELF runner with `/pkg/bin/appd`. The process lifecycle is configured for `HEART_TRANSPLANT`.

## Library Modules

The appd library exports:

- `broker`: service publication, discovery, singleton lookup, user-scoped singleton lookup, consumed-service binding, and broker snapshot/restore.
- `checkpoint`: appd state snapshots and registry validation.
- `debug`: app process debug registry.
- `device_registry`: parented device node registration, typed hardware-resource
  leases, ownership validation, lifecycle state, and post-order unregister.
- `driver_manager`: the appd-internal generic device coordinator, driver index,
  bind rule matching, ranked candidates, canonical topology, host grouping,
  package acquisition and recovery,
  recovery exclusions, driver package IDs, and lifecycle log.
- `guest`: guest-facing readiness, resolver, state, launch, and update helpers.
  Heart-transplant archive inspection, runner-adapter gating, and migration
  coordination are split so non-ELF future runners must register an adapter
  before appd stages or transfers ownership.
- `kernel_services`: publication of kernel-provided services into the broker.
- `lifecycle`: appd-side `DriverLifecycle.PrepareStop` helper.
- `manager`: app-manager FIDL handling for web install entrypoints, domain
  association lookup, and well-known reload/cache mutation.
- `manifest`: no-std app manifest model and protobuf decoder.
- `namespace`: startup namespace construction and app storage namespace helpers.
- `opener`: manifest opener registration plus runtime binding records for the app opener service.
- `permission_store`: shared system/user permission grant model used by appd.
- `platform_config`: platform policy model and policy decisions.
- `policy`: permission and FIDL capability checks.
- `registry`: published interface registry.
- `recovery`: D1 crash/rebind recovery budget model.
- `recovery_image`: cached D1 executable/library VMO metadata for driver restart.
- `routing`: retained provider endpoint routing and replay helpers.
- `runner`: ELF parsing, runner registry, package image resolution, kernel launch operations, and runner policy.
- `waves`: startup wave planning, readiness gates, and orchestrated process launch.

## Service Broker

`AppdBroker` owns the current capability registry. It supports:

- publishing explicit `ExposedService` entries;
- publishing all exposed services declared by a manifest;
- freezing and restoring broker state for migration;
- querying visible/bindable interfaces by protocol and metadata;
- singleton and user-scoped singleton lookup;
- binding manifest-declared consumed services by creating kernel channels and
  returning client/provider endpoints.
- publishing multiple service instances from one driver package with stable
  `device.node_id` metadata.

Visibility is currently enforced as:

- public: visible to all clients;
- private: visible only to the provider package;
- domain-shared: visible to packages that share the same package-name domain prefix;
- unspecified: hidden.

Binding is declaration-first: appd only grants services listed in the caller's
`services_consumed` manifest entries. Provider visibility and bind permissions
are still enforced, and capability metadata is intersected with any
consumer-declared capability scope before endpoints are minted. Each consumed
capability lists explicit method dependencies with required or optional
`link_type`. Required methods must be present in the provider's exposed method
ordinals; optional methods are granted only when present. Empty method lists
request no provider methods, so appd does not mint a provider/client channel for
that capability. Each bound capability carries only the allowed method ordinals
and granted permission values, and userspace providers enforce those ordinals in
their dispatch loops. For driver-provided services appd keeps its provider
endpoint open and sends a duplicate endpoint to the active driver manager, so
requests can queue while a D1 replacement is launched and retained endpoints can
be replayed after readiness.

## Manifest Model

The current manifest model mirrors `idl/bexos/app/manifest.proto`. It includes:

- package identity, display name, structured package version, multi-version
  policy, and minimum BexOS ABI metadata;
- package kind, defaulting to application, with library packages rejected as
  launch targets;
- library dependency declarations with package name, required ABI version,
  optional version requirement, and optional mount alias; dependency resolution
  accepts `package:version` selectors and partial package/version matches,
  rejecting ambiguous matches;
- process list;
- package/process structured permission declarations with required/optional
  requirement, scoped values, and usage descriptions;
- exposed services and declaration-scoped consumed services;
- resource groups;
- driver metadata and bind rules;
- component config schema;
- ELF runner options encoded through `google.protobuf.Any`;
- lifecycle update strategy with default restart, optional heart transplant, migration timeout, and preparation timeout;
- background job declarations with job ID, target component/process,
  one-shot/periodic timing, flex window, power/network constraints, persistence,
  and execution budget. Installed package records carry a durable package
  instance ID derived from the signed manifest/install source; jobd stores this
  ID with every job and purges jobs after uninstall or replacement.

The legacy string permission manifest shape is no longer accepted; source app
manifests use prototxt `permissions { ... }` blocks. The manifest decoder is
implemented manually without a protobuf runtime so boot components can decode compiled
`.bexmanifest` records without relying on a std protobuf runtime.

Disk launches resolve declared library dependencies through the app registry and
active package pins before opening the selected archive through `vfsd`. Appd
requires each dependency to request a nonzero ABI version and match a declared
`library_exports` ABI from a `package_kind: LIBRARY` package. It mounts `/pkg`,
package/user-scoped `/data`, and a fresh per-launch `/tmp` root before
dependency roots, exposing dependencies as `/deps/<mount_alias-or-package-name>`.
Missing, ambiguous, non-library, or ABI-incompatible required library packages
fail launch before the target process starts. Early BootFS wave services can
load declared library code from BootFS but do not yet receive dependency
namespaces because they start before the persistent package store is mounted.

The registry stores package records by package plus structured `SemVer`.
Single-active installs update an active pin and keep the previous version as a
rollback target. Protected single-active installs enter probation; successful
launch marks the active pin healthy. Parallel execution uses version-isolated
app data, while shared-storage multi uses shared package data.

Registry records also carry an accepted heart-transplant generation. Older
checkpoint/db rows default this value to zero. Service replacement commits are
atomic over the selected archive path, manifest metadata, archive content hash,
and accepted generation. During boot, BootFS and system package manifests are
merged as baselines, then durable registry selections win after the persistent
store opens; appd reopens the selected content-addressed archive ID and raises
the running service generation floor from the committed record. Protected
service updates fail while durable stores are unavailable, so appd does not
report an in-memory-only replacement as complete.

Manifest processes can declare opener intent filters under `handles`. The
current decoder supports custom schemes, HTTPS domains, MIME types including
`type/*` wildcards, and provided interface names. HTTPS domain declarations are
kept in the model but are not registered as automatic URL handlers unless a
local verification hook marks the registration verified. Appd keeps a
domain-association cache for verified domains. Direct app URL installs now
reject non-HTTPS URLs, credentials, fragments, and invalid authorities through
the shared distribution parser, then perform a best-effort well-known refresh
for the URL host after a trusted install. The shared distribution crate now
also provides the buildable `//lib/distribution:distribution_live`
rustls/netstack HTTPS transport for callers that opt into live networking. The
live transport is a compatibility wrapper over `//lib/net:net_secure`: it
consumes Netstack and `TlsTrustManager` channels, exports trustd's TLS REDB root
bundle into rustls, verifies hostname/SNI during the TLS handshake, and returns
bounded HTTP/1.1 bodies. Appd's focused unit tests continue to use injected
fetchers so transactional behavior can be exercised without external
networking; the current QEMU product image links the core distribution client
unless live app/update fetching is explicitly selected.

## Domain Associations

Appd exposes the public singleton `bexos.app.manager.AppManager` beside Opener
and VersionManager. `ReloadWellKnownForDomain`, `GetDomainAssociation`, and
`InstallAppFromUrl` are wired. Reload canonicalizes the supplied domain to a
lowercase host without scheme, port, path, or trailing dot, parses
`.well-known/bexos-manifest.json` into a `DomainPolicyRecord`, updates the
migratable in-memory cache, and replaces the matching persistent entry in
`domain_associations.redb` when the system store is mounted. Cache values are
manual protobuf bytes keyed in the `domain_policies` table by canonical domain.

Reload also revisits installed manifest opener registrations for the refreshed
domain and revokes or marks HTTPS domain handlers based on the refreshed policy.
The host/debug command path is wired through `debugd` and `bexctl`, but the
production appd fetcher currently reports network-unavailable until the guest
HTTPS client work lands. A failed best-effort well-known refresh after a
trusted URL install reports the association status without undoing the install.

## Packages And Lifecycle

Appd tracks versioned package records and active pins for single-active
packages. Active pins are first-class checkpoint records and are mirrored into
the initialized `pinned_apps.redb` database for `SysStateV1.active_slot`
(`slot_a` or `slot_b`). Protected single-active service launches remain in `PROBATION`
after startup readiness and are promoted to `HEALTHY` only after the configured
survival window. The default platform policy is 60 seconds of probation, a
60-second sliding crash window, a threshold of three exits, and a one-second
restart delay; both QEMU platform configs set those values explicitly.

Completed commands remain stopped. Automatic restart requires a service
entrypoint and tracks its UID as well as package and process name. Selected
SysUI/UserUI processes use the graphical session supervisor for recovery.
The appd watchdog polls protected lifecycle-managed process handles with
nonblocking kernel `WaitMany`, uses monotonic time for probation and crash
windows, excludes job workers, cleans dead launch handles/bindings/routes, and
relaunches eligible services after the restart delay. Probation exits and the
third healthy exit in the crash window mark the failed pin `CRASH_LOOP` and
restore the rollback target when one is present; without a valid fallback the
pin stays crash-looped and appd stops restarting it. Hang/heartbeat detection is
not implemented yet.

`VersionManager.ListVersions` matches exact package IDs, reports allocated
archive bytes for disk-backed records through vfsd, and reports zero bytes for
BootFS-only records. `PruneInactiveVersions` keeps the active version plus the
newest requested inactive SemVer versions, protects a probation rollback
target, deletes archive files, removes matching registry records, and reports
the actual allocated bytes reclaimed. Installs now fail when archive persistence
fails, and uninstalls delete storage before dropping the registry record.

## Shared Vaults

App manifests can declare shared vaults by name, access mode, and explicit
allowed peer package prefixes. At launch, appd validates those declarations
before asking vfsd for a directory handle and mounting it as
`/shared/<vault_name>`.

Web-style packages such as `com.google:maps` are limited to their authoritative
domain prefix by default. Sharing with another web domain requires cached
bidirectional `trusted_peer_domains` association records. Local `system:` and
`user:` apps do not inherit a domain identity; they can share only when both
manifests declare the same vault and explicitly allow the peer prefix.

Launch UID controls storage scope. Apps running as `SYSTEM_UID` (`0`) receive
only system-only shared vaults whose peers are `system:` prefixes. A system app
that needs to share with a user app must be launched in that user's context, so
the bridge uses that user's per-user shared-vault backing path rather than a
global directory.

## Openers

Appd exposes the public singleton `bexos.app.opener.Opener`. Apps bind it
through the broker like any other consumed service, and appd retains the
provider side of those channels in its migratable runtime state.

The opener registry currently has in-memory system/user handler tables plus
user defaults. BootFS and installed package manifests register system-scope
handlers; uninstall removes matching handlers and defaults. User callers resolve
against user defaults and user handlers first, then fall back to system handlers.
System callers resolve only against system handlers.

Resolved opener calls launch or reuse the selected process and send a small
opener payload to the target's manager channel. URL opens transfer URL bytes,
file opens transfer the VFS file channel plus MIME type, app opens transfer the
argument vector, and preferred-interface opens transfer the supplied server
channel. When multiple handlers match and no default exists, appd returns
`PROMPT_PENDING_USER`; there is not yet a picker UI.

## Runners And Launch

The runner subsystem currently includes:

- `ElfRunner`;
- `ParsedElf` and `ElfMapping`;
- `RunnerRegistry`;
- `PackageImageResolver`;
- launch request/result types;
- fake and FIDL-backed kernel operations;
- policy checks for native ELF trust tiers.

The ELF runner builds each launched process below the restricted root VMAR
returned by `CreateProcess`. The executable image receives an image arena with
per-`PT_LOAD` child VMARs carrying the exact final segment permissions. Native
libraries receive per-library arenas and per-load-segment children. TLS, stack,
and stack-guard regions are separate VMARs; the guard page is intentionally left
unmapped. The legacy flat VM-space calls remain available for compatibility,
but normal app loading uses VMAR-relative operations. If any partial load fails,
appd destroys the constructed child VMARs and terminates the incomplete process
before returning the launch error.

The executable image and manifest-declared library graph use the hermetic BexOS
architecture-selected shared-library ABI. Shared parsing and linking live in the
`no_std` `//lib/elf` crate, with AArch64 and x86_64 relocation modules. Library package manifests declare `library_exports`
with an ABI version, export path, symbol prefix, and SONAME. Appd resolves those
exports recursively through the package registry, deduplicates resolved
package/export/ABI identities, rejects cycles, and gives the runner deterministic
dependency-first library metadata.

Library exports now carry an explicit kind. Omitted kind remains `NATIVE`, so
existing ELF packages keep their behavior. `WASM_COMPONENT` exports are resolved
only for WASM runner `component_imports`, are mounted through `/deps/<alias>`, and
are passed to the trusted runner as component payload VMOs rather than native
linker metadata. Appd rejects missing exports, ABI mismatches, dependency cycles,
count/depth/aggregate-size limit violations, and attempts to satisfy a component
import with a native library. Running WASM instances retain the resolved dependency
identities until relaunch or application transplant.

The loader validates each `DT_SONAME` against the selected manifest export and
requires every `DT_NEEDED` SONAME to match a directly declared dependency. It
parses SysV and GNU hash tables, `DT_INIT`, `DT_INIT_ARRAY`, `RELA`, and packed
`RELR` metadata with bounded table checks. Supported AArch64 relocations are
`RELATIVE`, `ABS64`, `GLOB_DAT`, `JUMP_SLOT`, `DTPMOD64`, `DTPREL64`, and
`TPREL64`. x86_64 supports `RELATIVE`, `64`, `GLOB_DAT`, `JUMP_SLOT`,
`DTPMOD64`, `DTPOFF64` and `TPOFF64`, with Variant II TLS below FS.
Relocated TLS templates initialize every thread. Unresolved weak symbols bind to zero, unresolved strong symbols fail
launch, and symbol-version requirements, IFUNC/IRELATIVE, TLSDESC, COPY
relocations, and text relocations are rejected. Library mappings use
span-aware page placement, final W^X permissions, and read-only `PT_GNU_RELRO`
pieces after relocation.

Runtime linker metadata is delivered separately from component config. Startup
ABI version 7 adds an optional trace producer descriptor while preserving the
v6 named namespace vector, optional config-control endpoint, and `linker_data`
fields. Appd provisions one 2 MiB trace VMO per launched native process,
duplicates the registry-side handle, queues registrations before traced is
ready, and registers producers through traced's privileged `TraceRegistry` once
available. The `linker_data` VMO uses architecture-tagged `BXLINK03` and contains
exported manifest-prefix symbols, constructors, and the aggregate static TLS
layout. ARM retains readers for `BXLINK01` and `BXLINK02`; x86 rejects these
untagged legacy formats.
`Startup::receive` installs linker data and runs constructors once in
dependency-first order before returning to application code.

Std-linked Rust applications that use `std::net` must declare a consumed
`bexos.net.Netstack` service in their prototxt manifest and call
`bexos_libc::install_startup(&startup)` after receiving the startup message.
The libc shim stores that grant and maps POSIX socket calls onto netstack FIDL
control protocols plus kernel SOCKET handles for TCP stream reads and writes.

Boot resources remain separate from service grants and named namespaces. Device
MMIO, progress blocks, worker job-control handles, migration channels,
component config, and runtime linker-data VMOs are passed as role resources.
Package-store launches receive `/pkg`, `/data`, `/tmp`, shared-vault, and
`/deps/...` directories as explicit `NamespaceEntry` values instead of relying
on their position in `Startup.resources`; libc installs `std::fs` mounts from
that named namespace table. Component config uses `config`/`config_len`;
shared-library linker metadata uses `linker_data`/`linker_data_len`. Service
dependencies such as `vfsd`, `usersd`, TEE manager, netstack, TLS trust, and
kernel capability services are represented as named startup service grants when
they are declared by the process manifest.

Service-grant descriptors are backward-compatible with the original five-field
shape and now optionally carry caller package ID, caller UID, and foreground or
background context. `jobd` requires those identity fields before accepting
schedule requests, so dynamic job specs can be clamped to the caller's manifest
job declaration.

Appd exposes the privileged singleton `bexos.app.worker.WorkerLauncher` for
`jobd`. Worker launches reuse appd's normal disk launch path, service grants,
namespace construction, package dependency resolution, and UID handling, with a
job token plus `JobControl` channel injected into the worker startup resources.
Job-only launches validate that the target component names a manifest process
and that a manifest job declaration targets that process.

After the VFS handoff, appd installs its persistent redb stores for package
registry, permission declarations, opener registrations, and domain
associations. `WorkerLauncher.GetJobDeclarations` serves decoded signed
manifest job declarations plus the installed package instance ID from that
registry. `WorkerLauncher.StopWorker` is restricted to the jobd binding, matches
by package/UID/job token, calls the kernel privileged termination path for the
worker process, closes owned launch handles, and is idempotent from jobd's
perspective. Appd also preserves WorkerLauncher bindings and package-policy
watcher channels through heart transplant.

The current platform config allows native ELF only for allowlisted platform packages. The default runner tier is WASM, but the QEMU bring-up product explicitly allowlists platform/D1 services.

## Startup Waves

Startup waves are declared in package manifests and in `device/virtual/qemu/base/aarch64/bootfs_manifest.prototxt`. Appd plans launch order with readiness gates and process references.

The current QEMU order is:

- wave 0: PCI root and PL011 UART;
- wave 1: NVMe;
- wave 2: BexFS and archivefs;
- wave 3: diskimage, vfsd, debugd, updated;
- wave 4: powerd and usersd;
- wave 5: netstackd and prefsd when storage preinstalls are available;
- wave 6: timed when storage preinstalls are available;
- wave 7: jobd.

Keychaind is published as a dormant lazy provider instead of launching in a boot
wave. Appd retains its package and capability records, and the first authorized
Keychain service connection activates the `keychaind` singleton.

## Lifecycle And Debug

The app lifecycle FIDL surface supports app listing, bundle installation,
uninstall, launch, optional `RequestPermission`, migration begin/status,
process progress, privileged component-config get/set/reset and lock/unlock,
and explicit-UID preference management. Config
mutations resolve package selectors through the app registry, use a
generation-checked compare-and-swap contract, validate the supplied `BEXCFG`
snapshot before accepting it, and refuse mutations while a migration task is
pending. Accepted overrides are stored by exact package key so parallel
versions remain independent. Appd registers schemas and packaged baselines
with prefsd; startup receives the resolved read-only VMO and an optional live
config endpoint. Prefsd merges user preferences and explicit locks, coordinates
registered receivers before committing writes, and preserves durable generations.
Older receivers use the new values on their next launch.
`RequestPermission` currently auto-grants declared optional permissions after
validating the caller package/process/UID, requested values, consumed service
name, consumed capability name, explicit method scope, and provider capability
permission annotation. Preference bindings additionally carry the exact package
key so parallel versions resolve their own assembly and operator layers.
The request persists the grant synchronously, delivers a
duplicated provider endpoint with the same method/value/caller metadata used by
startup grants, retains the original provider endpoint in appd, and returns the
live client endpoint as `granted_handle`. Provider crashes keep the client
channel valid; when the same compatible provider republishes, appd replays a
duplicate retained endpoint to the provider manager. Ambiguous selectors return
`INVALID_ARGS`, unavailable providers return `NOT_FOUND`, undeclared or
locked-user requests return `ACCESS_DENIED`, and durable-store failures return
`STORAGE`. The app debug FIDL surface supports process listing.

Runtime service access also includes appd's ServiceDirectory surface. A caller
receives ServiceDirectory through a declared consumed-service grant, then uses it
to discover and connect to provider capabilities after startup. Appd authenticates
the caller from the appd-issued binding and applies consumed-service, capability,
permission, method-filter, and UID checks before activation. Discovery does not
launch dormant providers; a successful connection request does. Lazy startup
grants, runtime connections, and optional permission handles share the same
activation path and retain endpoint recovery metadata.

Permission declarations and UID 0 grants live in appd's system permission DB at
`/data/system/bexos.platform.appd/permissions.redb`. Nonzero UID grants live in
`/data/users/<uid>/permissions.redb`, opened through
`VfsManager.GetUserHomeDirectory`, so the store exists only while that user's
encrypted image is mounted. Nonzero-UID launches and permission requests are
refused while the user is locked. Unlock loads and reconciles that user's
grants; lock/delete purges cached grants and revokes live per-user optional
capability routes. Legacy nonzero-UID rows in the system permission DB are
scrubbed during system permission store reconciliation.

`debugd` uses related wire protocols for external QEMU/developer control,
including bundle install, URL install, component-config get/set/reset, and
well-known reload routing. `bexctl config get PACKAGE [-o PATH]` reports the
current generation and can write the current override snapshot. `bexctl config
set PACKAGE EXPECTED_GENERATION NAME=TYPE:VALUE...` submits explicit scalar
assignments such as `mtu=u32:1400` and `use_nts=bool:true`; `bytes` values use
hex digits. `bexctl config reset PACKAGE EXPECTED_GENERATION` restores the
operator layer to the packaged baseline, removes runtime locks, and advances
the operator revision; user assignments and product locks remain intact.
`bexctl prefs get/set/reset PACKAGE --uid UID` accesses the user layer.
Appd remains the authority for
app/package state and domain-association state.

## Migration Support

Appd currently has snapshot, restore, replacement archive, opener
registry/binding state, AppManager binding state, WorkerLauncher binding state,
component-config override records, permission state, retained optional
permission routes, usersd watcher state, domain-association cache state, and
lifecycle settings for heart transplant. The migration model uses generation
numbers, explicit control channels, timeout fields, and handoff state rather
than ad hoc restarts. During appd self-transplant, pending replacement
descriptors are serialized for activation but are not advertised as
source-owned resources to preserve; the kernel handover owns the candidate
process descriptors. Prefsd separately migrates authenticated preference
clients, user watchers, schema/policy state, and live transaction recovery.
Verified replacement archives are written and synced before the kernel migration
clock starts. Registry activation selects the staged generation only after
successful cutover. Boot archive discovery ignores generation archives and uses
the durable registry to select committed replacements; rejected candidates cannot
become preinstalled packages through directory enumeration. Other archive
discovery resolves signed manifest package keys.

See [preference validation](services.md#preference-validation) for the host and
QEMU coverage of configuration resolution and retained clients across cutover.

## Debug terminal identity

Debugd can bind a scoped opener through its privileged lifecycle channel. System
UID 0 uses system preferences; nonzero UIDs must be unlocked and use the existing
user-to-system preference fallback. Debugd verifies the user's password before
requesting this binding. Provider reuse includes package, process, and UID,
preventing a user terminal from being routed to another user's process.

## Locale startup handoff

Source integration is present; the local implementation and acceptance plan
remain incomplete. See [RFC 0066 current gaps](rfcs/0066/CURRENT.md#current-gaps-in-the-approved-local-scope).

Startup version 10 appends an optional locale descriptor containing a read-only
CLDR VMO, length, data generation, and encoded settings snapshot. The receiver
has frozen decoders for versions 6–9 in addition to the existing versions 2–5.
Appd registers the locale preference package before starting localed and resolves
application locale state through localed's appd-only startup binding. Early
services start without the descriptor; localed itself does not depend on its own
provider. A pending descriptor owns its duplicate until startup transfer succeeds,
so a failed launch closes it. See [localization](localization.md) for native and
WASM consumption and the [RFC status](rfcs/0066/CURRENT.md) for checks.
