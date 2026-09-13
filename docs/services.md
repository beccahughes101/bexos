# Services

## Appd

Appd is documented separately in [Appd And Userspace](appd-userspace.md). It is the bootstrap service and central policy/broker component.

## Font Provider (`fontd`)

Package: `bexos.service.fontd`; Bazel targets live under `//services/fontd`.
The workstation starts it in wave 5 and packages both the boot ELF and a
replacement archive. Its prototxt manifest consumes vfsd and usersd and exposes
`bexos.fonts.FontProvider` public resolve/fallback methods plus the separately
permissioned `InstallUserFont` method.

`fontd` indexes the five pinned system-image fonts and lazily indexes the active
user's encrypted home font directory. It validates bounded SFNT/TTC data,
deduplicates canonical VMOs by SHA-256, returns only transfer/read/map handles,
and migrates client channels, service/storage endpoints, the user watcher,
indexes, and live VMOs during heart transplant. Configured remote misses now
queue through pkgd; default products have no remote mappings and guest OCI
acceptance remains outstanding. See
[Local Fonts](fonts.md) and [RFC 0063 current state](rfcs/0063/CURRENT.md).

It also exposes the public singleton `bexos.app.opener.Opener` for URL, file,
direct-app, and preferred-interface opening. Handler declarations come from app
manifest intent filters and are resolved through appd's system/user
opener registry.

Appd's device registry also exposes USB, I2C, and SPI descendant registrar
capabilities used by `usbd`, `i2cd`, and `spid`. USB interface nodes are
registered as descendants with `BUS_CONTROL` resources so class drivers receive
scoped interface channels rather than xHCI MMIO, interrupt, or IOMMU handles.
I2C and SPI peripherals are registered from static topology with scoped
`BUS_CONTROL` channels bound to a single address or chip select.

Appd also exposes `bexos.app.manager.AppManager` for web install and domain
association controls. The current cache is backed by
`domain_associations.redb` table `domain_policies`, keyed by canonical domain
with protobuf-encoded `DomainPolicyRecord` values.

Appd also exposes the privileged singleton
`bexos.app.worker.WorkerLauncher`. `jobd` uses this service to launch declared
background worker processes with the caller UID, a job token, and an injected
`bexos.job.JobControl` channel. The same surface lets `jobd` fetch decoded
signed job declarations with the installed package instance ID, stop a worker by
package/UID/job token, and subscribe to package install/update/uninstall policy
events.

## prefsd

Package: `bexos.service.prefsd`; archive and replacement targets live under
`//services/prefsd`. It starts in wave 5 after usersd and storage availability.

Prefsd owns typed per-user configuration writes, explicit policy locks,
alternating durable wrapped-BEXCFG snapshots, generation checks, and
asynchronous live receiver coordination. Appd registers authoritative installed
schemas and provides caller-bound service endpoints. Ordinary applications
access their own package/UID; explicit-UID management is brokered through
privileged debugd.

Prefsd also exposes the public `bexos.ui.theme.ThemeManager` for the
config-only `bexos.ui.theme` package. `GetTheme` returns the caller UID's
resolved theme metadata plus a read-only stylesheet VMO. `WatchTheme` registers
a bounded observer channel and receives the same stylesheet payload after a
durable theme-affecting preference commit. Theme observers are included in
prefsd heart-transplant state so shell theme redraws survive service
replacement.

Nonzero-UID records exist only inside the unlocked encrypted user home;
lock/delete clears cached preferences and revokes user connections. UID 0 and
operator policy use prefsd's system data directory. Compatible schema versions
share preferences; incompatible schemas start from assembly/operator defaults.
The service includes bounded migration records and transfers authenticated
clients, config observers, theme observers, schemas, policy, generations, and
pending transaction state through heart transplant. Appd and prefsd declare an explicit one-second service
cutover budget in their prototxt manifests so x86_64 TCG acceptance keeps the
same handover semantics without tripping the 150 ms default jitter budget.

### Preference validation

The affected host suites cover shared schema constraints and legacy fingerprints,
partial overlays, locks and resets, generation conflicts, authenticated package
and UID bindings, compatible package versions, interrupted durable writes,
recovery, receiver coordination, and migration records:

```sh
bazel test //lib/component_config:tests //lib/ui/theme:tests \
  //services/prefsd:prefsd_tests \
  //lib/userspace:userspace_tests //tools/assembly:assembly_tests \
  //services/appd:appd_tests //services/debugd:debugd_tests \
  //lib/debug_wire:debug_wire_tests //host/debug_client:debug_client_tests \
  //tools/bexctl:bexctl_tests \
  //drivers/d1/storage/bexos/bexfs:bexfs_tests \
  //drivers/d1/storage/bexos/bexfs:bexfs_async_tests
```

The QEMU acceptance scenario uses generated live bindings and a retained public
preference connection. It exercises multiple receivers, a legacy instance,
rejected and timed-out prepares, privileged CLI transport, product/runtime locks,
operator overlays, encrypted-user isolation and lock/unlock, both prefsd and appd
heart transplants, and persistence after reboot:

```sh
bazel test --config=e2e \
  //testing/e2e/qemu/preferences:preferences_e2e_test_aarch64 \
  //testing/e2e/qemu/preferences:preferences_e2e_test_x86_64
bazel run @rules_rust//:rustfmt
```

## jobd

Package: `bexos.service.jobd`

Targets:

- `//services/jobd:jobd`
- `//services/jobd:jobd_elf`
- `//services/jobd:jobd_tests`
- `//services/jobd:jobd_archive`
- `//services/jobd:replacement_archive`

`jobd` is a wave 7 storage-preinstalled std/Tokio service with
HEART_TRANSPLANT lifecycle support. It exposes the public singleton
`bexos.job.Scheduler`, consumes appd's privileged `WorkerLauncher`, VFS,
powerd, netstackd, timed, usersd, and Clock services, and checks provider state
before running due work.

The in-memory scheduling model lives in `//lib/job_store` and has deterministic
record encoding plus redb host/BexOS-std storage adapters. Runtime scheduling
tracks owner package, owner UID, installed package instance ID, job state,
retry/run status, monotonic/realtime/waiting-anchor timebase, next-run time,
constraints, flex windows, persistence flags, execution budget, and worker
completion tokens. Schedule requests require caller identity from service-binding
metadata and are clamped against the package's decoded manifest job declaration;
package uninstall or instance-ID mismatch purges existing jobs.

Workers are launched through appd rather than directly by `jobd`. Appd validates
that the requested target component maps to a declared manifest process and, for
job launches, that at least one manifest job names that target. `jobd` opens the
system job store at `/data/system/bexos.service.jobd/jobs.redb` and opens
`/data/users/<uid>/jobs.redb` only for unlocked users. Only
`persist_across_reboots=true` records are written to redb; transient records
survive heart transplant but not reboot. Cold boot reconciles persisted
`RUNNING` records back to eligible scheduled work.

`jobd` subscribes to power snapshots, link state, time quality, user state, and
package policy watcher channels. Provider unavailability fails closed and marks
due jobs as `WAITING_CONSTRAINTS`; user lock moves nonzero-UID jobs to
`LOCKED_USER`, and unlock resumes them. Eligible work in the same flex window is
batched under one wake lease. `jobd` terminates or stops workers through appd on
completion, launch failure, and execution timeout, then persists the resulting
state and keeps periodic jobs on their UTC cadence without catch-up storms.

## usbd

Package: `bexos.service.usbd`

Targets:

- `//services/usbd:usbd`
- `//services/usbd:usbd_elf`
- `//services/usbd:tests`
- `//services/usbd:replacement_archive`

`usbd` is a wave 2 USB bus manager with HEART_TRANSPLANT lifecycle support. It
consumes xhcid's private `bexos.usb.host.XhciController` capability when an xHCI
controller is present and consumes appd's `DeviceRegistry` USB registrar
capability to publish USB descendants. It owns topology generations, class
policy decisions, interface claims, topology watcher channels, and the scoped
interface channels passed to class drivers.

See [USB](usb.md) for the current host-stack scope and validation status.

## i2cd and spid

Packages: `bexos.service.i2cd` and `bexos.service.spid`

Targets:

- `//services/i2cd:i2cd`
- `//services/i2cd:runtime_tests`
- `//services/i2cd:replacement_archive`
- `//services/spid:spid`
- `//services/spid:runtime_tests`
- `//services/spid:replacement_archive`

`i2cd` and `spid` are wave 2 D1 bus-controller services with
HEART_TRANSPLANT lifecycle support. They consume appd's private I2C/SPI
descendant registrar capabilities, load Bazel-compiled prototxt topology, and
register scoped peripheral nodes. Peripheral drivers receive only an
address-bound `I2cDevice` or chip-select-bound `SpiDevice` channel. Controller
management and deterministic fixture controls stay private.

The shared implementation is `//lib/i2c_spi`. It provides topology validation,
client helpers, bounded FIFO arbitration, lock leases, deterministic backend
behavior, and migration codecs used by both services. See
[I2C and SPI services](i2c_spi.md) for the current protocol, limits, migration
state, validation commands, and known hardware boundaries.

## debugd

Package: `bexos.driver.debugd`

Targets:

- `//services/debugd:debugd`
- `//services/debugd:debugd_elf`
- `//services/debugd:debugd_tests`
- `//services/debugd:replacement_archive`

`debugd` is a wave 3 service with `DEBUG_API_ACCESS`. The deployed ELF is a
std-linked Tokio daemon: `src/main.rs` builds a two-worker runtime, drives the
service loop asynchronously, and keeps the existing debug-wire frame format and
host client behavior unchanged. Its backend traits and frame handlers are async,
while the UART transport remains MMIO-backed with cooperative yield points
around bounded polling so migration traffic can still make progress.

It uses:

- `//drivers/d1/serial/arm/pl011` for guest-side debug transport;
- `//lib/debug_wire` for framing and request/response encoding;
- `//lib/update` for update upload handling;
- `//lib/migration` for live migration state.

The host client library is `//host/debug_client`, and the Clap CLI is `//tools/bexctl`.
See the [complete CLI reference](cli.md) for help, completions, output formats,
all diagnostic commands, and authenticated terminal use. Client RPCs, debugd
backends, live adapters, and CLI commands are organized into domain modules.

`debugd` bridges `bexos.shell.ShellProvider` and `bexos.tty.PtySession` to bounded
BXD1 polling. System shells require an explicit system selector; user shells
verify credentials with usersd before binding an appd opener to that UID.
Provider processes are reused only within the same UID. There is no production
shell package yet: missing providers return an explicit error. The shared
`//lib/tty` library supplies frontend controls and reusable provider session/line
discipline support. Active frontend handles and identity survive debugd transplant;
credential-bearing partial requests are excluded from snapshots.


Supported debug client operations include health check, process listing, app
listing, exec command, test app install/launch, app bundle install, URL install,
well-known reload, uninstall, update upload/commit flows, and TEE management proxy calls. The TEE path uses
typed debug-wire methods rather than shell-style exec commands for management
operations; exec aliases remain only for `tee.info`, `tee.apps`, and
`tee.update.status` diagnostics.

The host client library and `bexctl` expose `install-url URL` and
`reload-well-known DOMAIN`; debugd forwards both to appd's AppManager channel
after the appd handoff.

`bexctl tee` exposes TEE info, package-aware trusted-app listing, legacy
developer trusted-app install/uninstall, session open/close, command invoke,
TEE core update, and TEE update status.

## traced

Package: `bexos.service.traced`

Targets:

- `//services/traced:traced`
- `//services/traced:traced_elf`
- `//services/traced:traced_tests`
- `//services/traced:replacement_archive`

`traced` is a wave 3 std-linked Tokio service with trace control privileges.
Its package manifest exposes public `bexos.tracing.TraceController` and
system-privileged `bexos.tracing.TraceRegistry` singletons, declares runtime
dependencies on `bexos.lib.crypto` and `bexos.lib.net`, and declares
heart-transplant lifecycle support. The deployed ELF builds a two-worker Tokio
runtime around service-bound client polling, migration quiescence, producer VMO
drain, and cooperative yield points.

The library keeps the trace session state machine, shared producer registry,
bounded in-memory buffers, dropped-event accounting, Perfetto protobuf export,
and explicit legacy BexOS FXT export path. `TraceController` handles
`StartSession`, `StopSession`, and `GetStatus`; `TraceRegistry` handles
privileged producer register/replace/unregister. Runtime migration preserves
the control/migration channels, bound client channels, active trace session
metadata/events, producer mappings/read cursors, and the last exported trace
bytes.

The current host collection path is bridged through debugd's typed trace
methods and `bexctl trace`. See [Tracing](tracing.md) for commands, categories,
host client APIs, and limitations.

## vfsd

Package: `bexos.service.vfsd`

Targets:

- `//services/vfsd:vfsd`
- `//services/vfsd:vfsd_elf`
- `//services/vfsd:vfsd_tests`
- `//services/vfsd:vfsd_archive`
- `//services/vfsd:replacement_archive`

`vfsd` is a wave 3 std-linked Tokio service and participates in heart
transplant lifecycle. Its deployed ELF builds a two-worker runtime around the
direct VFS manager channel, drives request polling asynchronously with
cooperative yield points, and preserves initialized package-store, DiskImage,
BexFS, tmp, and unlocked user mount handles across migration. It implements the
VFS manager surface for:

- package store initialization;
- tmp manager initialization;
- package directory lookup;
- package archive read/write/delete/list plus logical and allocated archive
  byte attributes;
- per-user encrypted image create/unlock/lock/delete;
- user home, app-data, and shared-vault lookup while the user is unlocked;
- fresh per-launch tmp directory lookup;
- shared-vault directory lookup for appd-authorized `/shared/<vault>` namespace mounts.

User filesystems live under `vault/users/<uid>` as two encrypted DiskImage
slots plus a checksummed generation record. vfsd starts each user at 16 MiB,
monitors allocated utilization while unlocked, and performs a shadow resize on
the inactive slot once utilization reaches 60%. Preparing, committed, and stable
states are recovered deterministically on the next unlock. Locking a user calls
the mounted BexFS control channel to unmount only that volume, which closes
existing file and directory endpoints for that UID and drops retained key
handles.

The library has guest and migration modules for runtime integration and state
continuity. vfsd preserves the early MemFS manager channel across heart
transplant and brokers each app `/tmp` request to the D1 MemFS driver.
Filesystem mount, archive, and directory operations remain synchronous inside
the async server boundary. VFS request dispatch, archive/package-store, mount,
byte-counter, and failure paths emit trace events in the `vfs_io` category.
Versioned package archives are stored under
`pkg/<package>/<version>/pkg.bex`; vfsd creates those directories on write and
serves allocated-byte accounting without reading the archive into a VMO.

## usersd

Package: `bexos.service.usersd`

Targets:

- `//services/usersd:usersd`
- `//services/usersd:usersd_elf`
- `//services/usersd:usersd_tests`
- `//services/usersd:package_manifest`
- `//services/usersd:replacement_archive`

`usersd` is a wave 4 system singleton that starts after `vfsd`. It owns the
persistent redb-backed user registry at
`data/system/bexos.service.usersd/users.redb`, password-authenticator records,
volatile unlocked user session state, and user filesystem lifecycle calls into
`vfsd`.

The canonical user record is defined in `idl/bexos/user/v1/user.proto`. The
runtime control surface is `idl/bexos/user/manager.fidl` and supports user
list/get/create/update/delete plus lock/unlock. Password-created debug users
store an Argon2id/AES-256-GCM wrapped U-KEK; debug tooling generates the U-KEK,
salt, and nonce on the host and sends only wrapped material to the guest. On
unlock, usersd unwraps the U-KEK, passes it to vfsd in a short-lived VMO, then
zeros temporary copies. Lock is idempotent for already-locked users, and delete
locks before removing both image slots and state.

`debugd` exposes typed user operations through `bexctl users`:

- `bexctl ... users list`
- `bexctl ... users get UID`
- `bexctl ... users create --uid UID --name NAME --password PASSWORD [--display-name NAME]`
- `bexctl ... users update --uid UID --name NAME [--display-name NAME] [--disabled]`
- `bexctl ... users delete UID`
- `bexctl ... users unlock UID --password PASSWORD`
- `bexctl ... users lock UID`

## keychaind

Package: `bexos.service.keychaind`

Targets:

- `//services/keychaind:keychaind`
- `//services/keychaind:keychaind_elf`
- `//services/keychaind:keychaind_tests`
- `//services/keychaind:keychaind_archive`
- `//services/keychaind:replacement_archive`

`keychaind` is a std-linked Tokio system singleton exposed as a lazy provider.
Appd publishes `bexos.security.Keychain` as dormant after manifest validation,
excludes the process from startup waves, and launches the `keychaind` process on
the first authorized Keychain connection. Runtime and startup-grant connections
are delivered through the same lazy activation path, so concurrent clients share
the singleton and reconnect after idle exit starts a fresh instance. Keychaind
receives declared grants for `usersd`, `vfsd`, and `teed` when it launches and
keeps two normal-world redb vault namespaces:

- a system keychain for machine/system credentials at
  `data/system/bexos.service.keychaind/keychain.redb` on the general storage
  partition;
- per-user keychains that are only available when `usersd` reports the user as
  unlocked, stored as `keychain.redb` inside that user's encrypted home image.

Within each vault, records are keyed by alias only. User separation is provided
by the vault instance and encrypted-home location rather than by a `(uid,
alias)` record key. The shared `//lib/redb` wrapper provides host, no-std, and
BexOS std block-store adapters for redb-backed storage.

The FIDL surface is `idl/bexos/security/keychain.fidl` and supports
store/get/delete secret calls plus key generation/signing calls.
Hardware-backed Ed25519, ECDSA P-256, and AES-256-GCM operations are routed to
Trusty KeyMint through typed privileged `teed` clients. The normal-world store
contains aliases, characteristics, public material, opaque KeyMint blobs, and
encrypted secret envelopes, never hardware private-key material. Auth-bound
operations request a current Gatekeeper hardware-auth token from usersd's
private broker for each operation and return `ERR_LOCKED` without one.

The manifest declares runtime dependencies on `bexos.lib.crypto` and on the
`bexos.user.UserManager`, `bexos.service.vfsd`, and `tee_manager` services. The
guest binary depends on `//lib/crypto_client`; appd loads the crypto library
package, maps `lib/libbexos_crypto.so`, and passes resolved ABI linker data at
startup.

## trustd

Package: `bexos.service.trustd`

Targets:

- `//services/trustd:trustd`
- `//services/trustd:trustd_elf`
- `//services/trustd:trustd_tests`
- `//services/trustd:package_manifest`
- `//services/trustd:replacement_archive`

`trustd` is a wave 3 BootFS service with implemented heart-transplant lifecycle
support. The deployed ELF is a std-linked Tokio daemon: `src/main.rs` builds a
two-worker runtime and drives the service loop asynchronously while preserving
the public service names and FIDL wire behavior.
The QEMU BootFS image includes the selected public ecosystem bundle at
`/system/ecosystem/bexos.bundle` plus generated redb stores at
`/system/certs/tls_roots.redb` and `/system/certs/app_signing_roots.redb`.

The service exposes `bexos.security.trust.AppTrustManager` from
`idl/bexos/security/trust.fidl` and also exposes
`bexos.security.trust.TlsTrustManager` for read-only TLS/Web PKI root bundle
export. At startup it exports the TLS root redb bytes unchanged and parses the
app-signing root redb into its in-memory trust model, so `ValidateAppSigner`
uses the generated persistent root store. Validation now carries an explicit
signature algorithm, required trust tier, payload digest, signature, and signer
chain, returning the granted tier, root anchor id, leaf fingerprint, and
algorithm. Ordinal 1 remains readable by ordinary clients; enterprise-root
install, revocation update, enterprise-root removal, and enterprise-root list
are system-privileged mutation ordinals. Heart-transplant records preserve the
TLS root bytes, immutable app-signing roots, dynamic enterprise roots,
revocation generation, control and migration channels, and live App/TLS client
channels with method filters. The current verifier enforces prefix, tier,
validity, immutable-root revocation protection, rollback protection, chain-size
caps, Ed25519 and ECDSA P-256 package signatures, ordered DER X.509
leaf/intermediate/root path building, issuer signature verification, CA/basic
constraints, key-usage and code-signing EKU requirements, anchor selection,
and certificate/SPKI/serial revocation checks across the selected path.

Development root stores and the selected ecosystem bundle are generated by:

```sh
bazel build //ecosystem/bexos:public_bundle //ecosystem/bexos:tls_roots_redb //ecosystem/bexos:app_signing_roots_redb
```

Source roots live under `//ecosystem/bexos/dev/roots/pki` for TLS/Web PKI and
`//ecosystem/bexos/dev/roots/apps` for app-signing roots. Ecosystem profile policy,
TUF root metadata, direct-update public keys, and signer descriptors live under
`//ecosystem/bexos/{dev,prod}`. App root metadata and distribution policy are
source prototxt.

## netstackd

Package: `bexos.service.netstackd`

Targets:

- `//services/netstack:netstackd`
- `//services/netstack:netstackd_elf`
- `//services/netstack:netstackd_tests`
- `//services/netstack:package_manifest`
- `//services/netstack:netstackd_archive`
- `//services/netstack:replacement_archive`

`netstackd` is a wave 5 system singleton preinstalled on the QEMU `STORAGE`
partition. App-service launches it after disk drivers and the storage-backed
VirtIO-Net driver are bound, then publishes `bexos.net.Netstack`.

The deployed ELF is a std-linked Tokio daemon. Its entrypoint creates a
two-worker runtime with I/O and time enabled, then drives the service loop as an
async task while preserving the existing polling-oriented FIDL and packet-plane
behavior.

The service declares and consumes `bexos.hardware.ethernet.Device`, registers RX/TX shared
buffers, maps those VMOs, starts the Ethernet device, and pumps Ethernet
`FrameEntry` descriptors over the driver FIFO. It keeps RX buffers supplied,
copies outbound UDP frames into TX slots, consumes TX/RX completions, and maps
FIFO or VMO failures into `bexos.net.Status`.

The current data plane is dual-stack for configured IPv4 and IPv6. A
FIFO-backed smoltcp device drives TCP connect/listen state over the virtio
packet path and bridges established TCP payloads to kernel `SOCKET` stream
handles. UDP sockets also attach to smoltcp, so IPv4 UDP, IPv6 UDP, ARP, and
IPv6 neighbor discovery share the same packet path as TCP. DHCPv4 ACK packets
received on the packet path update the IPv4 lease; DHCPv6 remains out of scope.
Static IPv6, IPv6 prefix, IPv6 gateway, IPv6 DNS, IPv6 DoH bootstrap, and SLAAC
enablement are accepted as prototxt config fields. DNS cache entries are typed
A/AAAA records with expiry and resolver replies can return mixed IPv4/IPv6
addresses. Cache misses use configured resolver policy: UDP/53 for the default
QEMU profile, DoH bootstrap metadata for encrypted DNS, and strict DoH
fail-closed behavior when plaintext fallback is disabled and no secure
bootstrap route is configured.

`netstackd` is heart-transplant-capable. Its migration records preserve the
service control channel, migration channel, TLS trust service grant, client
channels, TCP/UDP/listener control handles, kernel stream handles, active
dual-stack and resolver config, DNS cache and pending DNS queries, ephemeral
port cursor, TCP socket metadata, and Ethernet FIFO/VMO handles plus live RX/TX
mappings. During quiescence the service stops accepting new client work and
drains observed FIFO completions before cutover. During activation it restores
preserved TCP endpoints/listeners over the retained Ethernet resources and
aborts replacement if any active socket cannot be restored.

## timed

Package: `bexos.service.timed`

Targets:

- `//services/timed:timed`
- `//services/timed:timed_elf`
- `//services/timed:timed_tests`
- `//services/timed:package_manifest`
- `//services/timed:timed_archive`
- `//services/timed:replacement_archive`
- `//lib/userspace_async:userspace_async`

`timed` is a wave 6 system singleton preinstalled on the QEMU `STORAGE`
partition. App-service launches it after storage package import and after
netstackd's wave 5 service is available.

The service consumes `bexos.net.Netstack`,
`bexos.security.trust.TlsTrustManager`, `bexos.kernel.Clock`, and
`bexos.kernel.SystemPrivileged` with the `SET_TIME` permission, and optionally
consumes `bexos.time.RtcHardware`. It exposes public singleton
`bexos.time.TimeManager`, uses SNTPv4 request/response processing with the
`sntpc` crate present in the build graph, persists the latest target time
quality record in `data/system/bexos.service.timed/time.state`, and accepts NTS
configuration through the existing `use_nts` API. When NTS is enabled, timed
uses `//lib/net:net_secure` and Netstack TCP ordinal 1 for NTS-KE over rustls
with ALPN `ntske/1`, negotiates NTPv4 plus AES-SIV-CMAC-256, consumes one-use
cookies, validates NTP extension framing, rotates returned cookies, and remains
fail-closed without SNTP fallback. Initial, failed, and manual time corrections
step realtime immediately; subsequent accepted network corrections up to the
configured one-second threshold slew at up to 500 ppm. On QEMU, the wave-0 PL031
RTC driver can provide startup UTC and accepts best-effort writes after manual
or network time acceptance. Accepted NTS sync records `ClockSource::NTS_SECURE`.
The `network_sync_enabled` component configuration defaults to true. Setting it
to false disables automatic and requested network synchronization; RTC validation
still runs at boot. The package-registry acceptance fixture uses this mode so
its clock prerequisite does not depend on a public time server. A retained time
quality record alone no longer establishes the current boot's kernel clock.

`timed` is std-linked and runs its service loop on a two-worker Tokio runtime.
The loop uses the shared `//lib/userspace_async` helpers around explicit BexOS
channels/RPCs, preserving the existing `TimeManager` FIDL wire ABI. Its
heart-transplant records preserve the control and migration channels, data
namespace handle, netstack, TLS trust, and RTC grants, client channels, config,
time quality, NTS association state including negotiated server/port, cookies,
exporter keys, and replay window, clock/set-time capability flags, in-flight
slew state, and the next sync deadline. The persisted `time.state` blob remains
the current on-disk format.

## net library

Package: `bexos.lib.net`

Targets:

- `//lib/net:net`
- `//lib/net:net_shared`
- `//lib/net:net_tests`
- `//lib/net:net_archive`
- `//lib/net:replacement_archive`
- `//lib/net_client:net_client`
- `//lib/net_client:net_client_tests`

`bexos.lib.net` is a field-replaceable SDK library package. Its manifest
declares `library_exports` for `/pkg/lib/libbexos_net.so`, and appd's
ELF loader resolves library dependencies generically instead of recognizing only
`bexos.lib.crypto`. The Rust client facade discovers `bexos.net.Netstack` and
`bexos.security.trust.TlsTrustManager` grants from process startup and binds the
shared-library ABI linker data.

The shared net artifact now links through the common BexOS libc/runtime support
instead of carrying private freestanding libc stubs, matching the std-linked
service path used by D2 applications.

## updated

Package: `bexos.service.updated`

Targets:

- `//services/updated:updated_elf`
- `//services/updated:package_manifest`
- `//services/updated:replacement_archive`
- `//lib/update:update`
- `//lib/update:update_host`
- `//lib/update:update_tests`
- `//lib/tuf:tuf`
- `//lib/tuf:tuf_tests`

`updated` is a wave 3 std-linked Tokio service that exposes
`bexos.update.UpdateManager`. It owns debug-upload staging, signed update
verification, app/service apply routing through appd, platform update routing
through the kernel, and TEE image routing through teed. It also exposes TUF
feed check, stage, and apply methods after the original upload/apply ordinals.
The shared `//lib/tuf` library verifies TUF root, timestamp, snapshot, targets,
delegations, expiration, rollback, hashes, and Ed25519 thresholds before update
targets are accepted.

`debugd` keeps the host-facing BXD1 and `bexctl update` surface, but proxies
those update requests to `updated`. Developer commands include:

```sh
bexctl --socket /path/to/debugd.sock update check [PACKAGE|TEE|KERNEL] [--all] [--stage] [--apply]
```

App feed targets are staged only after exact TUF length/hash verification and
then passed to appd's byte-install path. Kernel and TEE feed targets route
through the existing platform apply path. The feed manager persists trusted TUF
root/version rollback state across `updated` heart transplant; injected
repositories/transports remain test-only until the live netstack/TLS handoff is
completed.

## teed

Package: `bexos.service.teed`

Targets:

- `//services/teed:teed`
- `//services/teed:teed_elf`
- `//services/teed:teed_tests`
- `//services/teed:teed_archive`
- `//services/teed:replacement_archive`
- `//lib/userspace_async:userspace_async`

`teed` is a wave 3, heart-transplant-capable D1 service that exposes the
singleton `bexos.tee.TeeManager` protocol from BootFS. It is provider-neutral:
startup linker metadata selects exactly one signed ABI-v1
`libbexos_tee_driver.so` implementation, and `teed` fails closed on missing
symbols, ABI mismatch, probe failure, or Trusty transport failure. The Trusty
product pins `bexos.lib.tee_driver.trusty`; explicit emulated products/tests
pin `bexos.lib.tee_driver.software`.

`teed` owns TEE discovery, package/catalog metadata, stable public session IDs,
pending request state, command invocation, and TEE core update status. During a
`teed` or driver heart transplant it drops pending driver operations, binds and
probes the candidate library, reloads package-managed apps, reconnects
catalogued endpoints, preserves public session IDs, and reports in-flight calls
as unavailable. Secure-app private volatile state is not preserved.

`UpdateTeeCore` retains the provider-neutral dual-slot lifecycle state model.
Its status includes phase, active and pending slot, generation, rollback
availability, and reboot requirement. The QEMU Trusty backend rejects core
activation with access denied because no firmware slot writer is installed.
Firmware changes require `bazel run //third_party/trusty:refresh_image` and a
new QEMU instance. The dual-slot model remains available for future providers.

Trusted apps are durable `PackageKind.TRUSTED_APP` packages managed by appd.
Their manifests contain Trusty provider metadata, a 16-byte UUID, secure
monotonic version, embedded upstream apploader payload path, allowed Trusty
service ports, and protected/uninstallable policy. Appd replays active trusted
apps to `teed` after storage is ready, rejects update/uninstall while sessions
are active, installs candidates probationarily, marks them healthy only after
`teed` load/probe succeeds, and rolls back to the retained previous archive on
failure.

These package lifecycle interfaces do not imply dynamic loading support in the
current Trusty firmware. Its built-in catalog is discoverable, but dynamic
load/unload requests are explicitly denied until an upstream apploader is
integrated. The software backend exercises the package lifecycle in host tests.

The firmware includes upstream KeyMint, Gatekeeper, storage, AVB, AuthMgr
FE/BE, and the retained BexOS orchestrator, plus their required hwcrypto, hwbcc,
hwcryptohal, and system-state providers. Trusty storage uses teed's upstream
storage-proxy protocol handling over a private D1 virtio-console endpoint.
QEMU owns the RPMB helper and mutable state outside the saved firmware bundle.
The state survives instance reboots. AuthMgr owns
its upstream DICE and secure-service connection-authorization role; Gatekeeper
tokens authorize KeyMint directly. Secure ConfirmationUI remains unimplemented
until the platform can guarantee secure display and input ownership.

The D0 kernel side remains intentionally small:
`bexos.kernel.SecureMonitor.Call` is a privileged eight-register SMC bridge.
Trusty/TIPC framing, sessions, trusted-app metadata, policy, and update
semantics stay in the external D1 driver library and `teed`.

`debugd` receives a duplicated `teed` manager channel from appd during
boot, acknowledges the handoff, and proxies developer TEE operations through
the debug wire API and `bexctl tee`.

The current TEE FIDL surface uses fixed arrays for UUIDs (`array<uint8, 16>`)
and artifact hashes (`array<uint8, 32>`). TA payloads and TEE core images are
transferred as VMOs with explicit lengths. The current Trusty backend validates
the versioned BexOS dual-slot bundle, then explicitly rejects activation; the
software driver retains legacy raw-image compatibility for host tests.

`teed` is std-linked and runs on a two-worker Tokio runtime. Its Rust backend
and service traits are async, while the public `TeeManager` FIDL ABI remains
unchanged. Heart-transplant records preserve the service control and migration
channels, the private RPMB transport endpoint, connected client channels,
generation, package-visible trusted-app
state, sessions, and update progress, including slot and reboot-pending fields.
The Trusty driver exposes stage/activate/status operations through the external
C ABI; no provider-private state is serialized in the migration wire format.

## powerd

Package: `bexos.service.powerd`

Targets:

- `//services/powerd:powerd`
- `//services/powerd:powerd_elf`
- `//services/powerd:powerd_tests`
- `//services/powerd:powerd_archive`
- `//services/powerd:replacement_archive`

The power manager is wave 4, belongs to the `system` resource group, and has
`BEXOS_SYSTEM_PRIVILEGED`. It exposes public singleton
`bexos.power.PowerManager`, consumes required `bexos.kernel.SystemPrivileged`,
and optionally consumes `bexos.power.DevicePowerControl`,
`bexos.power.PowerTelemetry`, and `bexos.power.PerformanceControl` providers.
For the maintained QEMU AArch64 product it quiesces drivers to `D3_OFF`, then
calls the kernel PSCI primitive; if any device transition fails, already
transitioned devices are restored in reverse order, and suspend return or
firmware failure restores devices to `D0_FULL_POWER`. Successful reboot and
poweroff are terminal. `SUSPEND_TO_DISK` returns unsupported.

Snapshots report provider availability, raw battery/thermal measurements only
when providers exist, telemetry staleness, DVFS availability, and the applied
performance level. The default generic policy polls once per second, marks
battery low at 15%, throttles thermals at 85 C, applies minimum performance at
95 C, and uses 5 C recovery hysteresis. It applies reduced/minimum policy to an
optional DVFS provider and caps the built-in background resource group at
500/250 permille, restoring the captured baseline at nominal. QEMU does not
provide battery, thermal, or DVFS devices, so these fields report unavailable
unless a real provider is present.

## storage_verify

Package: `bexos.platform.storage_verify`

Targets:

- `//services/storage_verify:storage_verify`
- `//services/storage_verify:manifest`

`storage_verify` is placed in the system image and configured by the QEMU product with:

- `retry_limit = 3`
- `channel = "qemu"`

It depends on filesystem and block FIDL and provides progress accounting for storage verification flows.

## Shared Service Libraries

Several services rely on shared libraries:

- `//lib/debug_wire`: debug wire protocol framing and codecs.
- `//lib/migration`: versioned migration records and state helpers.
- `//lib/update`: signed update verification and metadata.
- `//lib/crypto`: host/test Rust cryptography helpers for password-wrapped
  keys, BLAKE3, and Ed25519 verification. It also builds
  `//lib/crypto:crypto_archive`, a signed `bexos.lib.crypto` library package
  containing the AArch64 C ABI artifact at
  `lib/libbexos_crypto.so`.
- `//lib/crypto_client`: guest no-std crypto ABI client. Runtime consumers call
  loader-resolved `bexos_crypto_*` symbols from `bexos.lib.crypto` instead of
  statically linking the crypto implementation.
- `//lib/net`: no-std and std host/test networking protocol helpers and the
  `bexos.lib.net` library package containing the AArch64 C ABI artifact at
  `lib/libbexos_net.so`.
- `//lib/net_client`: guest no-std net ABI client. Runtime consumers call
  loader-resolved `bexos_net_*` symbols from `bexos.lib.net` after appd loads
  the declared dependency.
- `//lib/redb`: no-std block-store adapter for redb, plus std host/BexOS
  file-backed block-store support.
- `//lib/userspace_async`: std/Tokio helpers for cooperative async channel
  receive, send, yield, and raw RPC calls over explicit BexOS IPC handles.
- `//lib/keychain_store`: alias-scoped keychain vault model with in-memory and
  redb-backed storage.
- `//services/teed`: D1 TEE manager with modular backend support.
- `//lib/userspace`: no-std userspace runtime support.
- `//lib/app_archive`: signed app archive format and zstd-capable decoding.
- `//lib/app_registry`: app registry model with optional redb-backed host implementation.
- `//lib/opener_store`: opener handler/default registry model with optional redb-backed host implementation.
- `//lib/user_store`: user registry model, manual protobuf encoding, and host redb persistence.
- `//lib/redb`: no-std/std boundary wrapper for redb-backed storage.
## Service Access

Appd grants service access only from manifest `services_consumed` declarations.
Required declarations must resolve to a visible provider at launch. Optional
declarations may be absent, but malformed capability scopes are rejected.
Consumer capability declarations narrow the provider's capability group and list
explicit required or optional method dependencies. Appd grants only requested
methods that the provider exposes, rejects missing required methods, drops
missing optional methods, and skips endpoint creation for capabilities with no
allowed methods. Service providers parse the granted method list from appd
binding metadata and reject dispatches for ordinals outside that list. True boot
resources, including MMIO, root stores, namespaces, migration channels, config
VMOs, and runtime linker-data VMOs, remain startup resources rather than services.

Keychaind is currently the only production lazy service. Its Keychain capability
is published dormant from the prototxt manifest, excluded from automatic startup
waves, and activated by the first authorized service connection. Multiple clients
share the singleton while it is running. The shared lazy-service controller tracks
accepted client channels and keep-alive tokens, asks appd for generation-tagged
idle shutdown after the final client drops, and preserves controller/client state
through heart transplant. If keychaind falls back to volatile storage, it holds a
keep-alive so idle exit cannot discard secrets.

## Graphical boot UI

The workstation adds three BootFS components with heart-transplant archives:

| Component | Startup | Authority and role |
| --- | --- | --- |
| D1 VirtIO-GPU | PCI binding, wave 1 | Owns scanout and DMA; grants exclusive presentation by endpoint and generation. |
| `splashd` | Wave 1, before storage mounting | Vello CPU animation, bounded progress reports, frozen-frame handoff. |
| `fontd` | Wave 5, after usersd and encrypted-home routing | Validated system/user font index and read-only VMO distribution. |
| `scened` | Wave 5, takeover after system readiness | Isolated Flatland sessions, CPU composition, acknowledged takeover and fade. |

Appd coordinates milestones and marks a successfully exited splash as a completed
boot service, preventing restart. Graphics readiness failures are nonfatal.
See [boot UI](bootui.md) for protocol, migration, validation, and current limits.


RFC 0064 now adds the centralized pkgd source implementation, including a
configured font miss path, asynchronous app installation, OCI/TUF verification
and a protected package-state endpoint. Default products contain no remote
registry roots or mappings. The complete guest/lifecycle acceptance remains
outstanding; see [package resolution current state](rfcs/0064/CURRENT.md).
The resolver still performs synchronous service and storage operations, and
the shared TLS root-service path needs VMO ownership cleanup. Live consumer
delivery and replacement have not passed guest acceptance. See the
[current gaps](rfcs/0064/CURRENT.md#current-gaps) for implementation limitations
and missing lifecycle assertions.
