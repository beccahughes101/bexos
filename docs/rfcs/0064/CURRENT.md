# RFC 0064 — centralized package resolution: current state

This implementation is in progress. The full three-phase acceptance plan has
not passed. The design and its remaining requirements are preserved in
[README.md](README.md). This page distinguishes source implementation from
validated guest behavior.

The implementation and validation gaps below were checked against the working
tree on 2026-09-13. None of the three phases has complete acceptance evidence.
See [current gaps](#current-gaps) before treating an implementation mapping as a
completion claim.

## Implemented interfaces and configuration

- `idl/bexos/pkg/resolver.fidl`: PackageResolver ordinals 1 (`ResolveArtifact`)
  and 2 (`ResolveBlob`), status-bearing optional VMO responses, bounded queries,
  SHA-256/BLAKE3 digests, and application/driver/font/firmware kinds.
- CredentialManager ordinals 1–3 provision tokens, provision a DER certificate
  chain and DER private key through VMOs, and remove credentials. Binding needs
  `BEXOS_SYSTEM_PRIVILEGED`; resolver replies never carry credential contents.
- AppManager ordinal 4 (`InstallAppFromArtifact`) returns installation status and
  allocated package identity. Existing HTTPS URL APIs retain their prior trust
  semantics. The new path queues pkgd calls and then uses the existing archive
  verification and installation path.
- `lib/pkg_client` provides asynchronous calls and pollable calls for service
  loops. `ReadOnlyVmo` owns and verifies the handle, rights, mapping, length and
  digest; cancellation/drop close owned resources. It adds no networking, TLS
  or credential implementation to its dependency closure.
- `services/pkgd/package/config.prototxt` is encoded by Bazel using
  `idl/bexos/pkg/config.proto`. Defaults: 256 MiB logical CAS quota, 64 MiB maximum
  payload, 16 queued artifact jobs and 64 waiters. At most two jobs execute
  concurrently, with repository refreshes serialized so their secure-state
  revision chains cannot invalidate one another. Products explicitly supply trusted root bytes, repository and
  origin allowlists, consumer/package-kind grants and named mappings. The default
  has no configured production repository, so it cannot fetch production content.
- `connect_timeout_ms` defaults to 10,000 and bounds each TCP-setup/TLS phase;
  `request_timeout_ms` defaults to 30,000 and bounds an HTTP request. Values must
  be nonzero and no greater than 120,000 and 600,000 respectively. These limits
  are part of the compiled configuration identity preserved across replacement.

## Resolution and storage implementation

The OCI transport discovers `tuf-timestamp` and sequential `tuf-root-N` manifests.
It validates role media types and digest/length descriptors, then fetches snapshot,
targets and payloads through SHA-256 blob URLs. Bearer challenges use configured
HTTPS token origins and repository-scoped pull requests. Redirect origins must be
explicitly configured; bearer tokens and mTLS identities remain scoped to their
registry authority (host plus optional port). TLS roots come from configured
`tls_roots_der` entries or the existing trust service. Connections use private
BexOS netstack bindings,
with h2/HTTP/1.1 ALPN, deadlines and bounded response framing.

`lib/tuf` verifies sequential root updates, threshold signatures over strict
canonical signed JSON, required expiration, exact metadata reference versions,
and bounded delegations with ordered/terminating lookup and path restrictions.
Accepted snapshot state includes every referenced metadata version, including
roles not yet downloaded, and survives interrupted refreshes. A root-authenticated
change of timestamp or snapshot keys resets those online-role counters for TUF
fast-forward recovery. The root version and time floor remain nondecreasing;
Trusty permits an online-role reset only in a newer root epoch.
The shared `refresh` operation is atomic. Pkgd separately commits each verified
root, timestamp, snapshot and targets transition before fetching subsequent
content. Its incremental `TargetSearch` fetches only matching delegations in
pre-order depth-first order, skips visited roles, honors terminating delegations,
and bounds each search to 256 roles, depth 8 and 8 MiB of targets metadata.
Delegation patterns support `*`, `?`, character classes and ranges.
Interrupted payloads retain those protections without granting artifact
access. Pkgd requires validated
RTC/NTS time and a nondecreasing persisted time floor; it never substitutes zero.

`services/pkgd/src/cas.rs` uses transactional redb tables through the existing
STORAGE filesystem and pkgd's system-data directory. Disk reads are rehashed;
corrupt content is deleted. Quota accounting and eviction apply to logical blob
bytes. The physical CAS file is capped at three times `max_cache_bytes` plus
16 MiB for database overhead; the separate public secure-state file is capped
at 256 MiB. Oversized backing stores and attempted growth fail closed. Artifact authorization lives
in secure repository state separately from deduplicated bytes. Cached digest
requests require an authorized persisted grant. `ResolveBlob` defaults to
cache-only in libpkg_client; explicit `allow_network_fetch=true` can recover
previously verified content using its saved SHA-256 descriptor and length even
if the original tag has moved. Unknown digests return `NOT_FOUND`. Named requests perform freshness verification even on a payload hit.

Payload transport writes directly into a private staging VMO, verifies the full
payload, unmaps writable memory and closes the writable handle. The canonical
handle retains duplication rights inside pkgd; clients receive only
`TRANSFER | READ | MAP`. Equivalent artifact jobs preserve each caller's expected
digest and authorization. Weak shared transfer entries coalesce concurrent
repository/digest/credential-generation transfers and drop abandoned I/O.

## Secure state and replacement implementation

`TeeManager.ExchangePackageState` (ordinal 14) is a pkgd-only capability. Generic
TEE endpoint opens reject `com.bexos.package-state`; the software emulator is not
a secure-state fallback. The Trusty orchestrator endpoint serializes authenticated
TP-storage operations with revision comparison and nondecreasing rollback/time
floors. Large public state is persisted in redb first and bound by its SHA-256
hash in Trusty; secure commit must succeed before newly resolved content is
released. Sealed token/key records live in Trusty. Volatile credentials use
private migration VMOs, not checkpoint bytes. An existing token and mTLS identity
must use the same sealing mode; a conflicting provisioning request is rejected.

Pkgd starts as a preinstalled STORAGE service in wave 7, after networking and
time services, using appd's normal service routing. Readiness does not open the
cache or contact registries; storage opens on demand. Pkgd declares heart
transplant and replacement archives. Source quiescence drops
transport futures and closes databases; `lib/redb::retained::RetainedStore`
keeps their externally owned filesystem channels open for adoption or recovery.
Records preserve client identities,
capabilities, pending queries, canonical VMOs and durable storage handles. The
target checks the complete blob, credential and job inventory, reopens storage
and restores credentials before restarting idempotent reads. Failed adoption leaves the source able to reopen its databases.

Appd queues configured driver and firmware mappings through package acquisition.
Driver packages enter the existing binding and driver policy checks. Firmware
currently uses process-free library archives; hardware-specific firmware
activation still needs acceptance coverage. Caller disconnection cancels pending
interactive app installation.

Fontd queues configured remote misses only with `allow_network_fetch=true`;
local matching retains priority. Known remote digests can use pkgd's cache.
Downloaded bytes are validated with the existing font parser and matched against
the requested family/style before forwarding the immutable VMO. Existing user
checks and format limitations remain in force.

## Validation evidence

RFC 64 has not passed guest acceptance. On 2026-09-13, the affected host batch
passed 20 targets; a separate corrected lifecycle harness executed and passed
four tests, bringing the validated host target set to 21:

| Host target | Passing tests |
| --- | ---: |
| `//services/pkgd:tests` | 21 |
| `//services/pkgd:migration_tests` | 4 |
| `//lib/tuf:tuf_tests` | 15 |
| `//lib/pkg_client:tests` | 2 |
| `//lib/pkg_client:dependency_policy_test` | Dependency policy check |
| `//services/appd:appd_tests` | 129 |
| `//services/fontd:fontd_tests` | 14 |
| `//services/teed:teed_tests` | 8 |
| `//services/vfsd:vfsd_tests` | 9 |
| `//services/netstack:netstackd_tests` | 18 |
| `//services/netstack:internal_tests` | 3 |
| `//lib/net:net_tests` | 5 |
| `//lib/userspace:userspace_tests` | 15 |
| `//lib/migration:migration_tests` | 9 |
| `//lib/redb:redb_tests` | 5 |
| `//lib/redb:retained_tests` | 1 |
| `//testing/pkg_registry:https_test` | 4 |
| `//testing/pkg_registry:font_test` | 1 |
| `//kernel/core:architecture_tests` | 2 |
| `//secure/orchestrator/trusty:package_state_test` | Protected-state checks |
| `//tools/image:assemble_bootfs_test` | Assembly checks |

Use `--test_tag_filters=` when combining these with `--config=e2e`: the profile's
default `requires-qemu` filter otherwise excludes host tests. The lifecycle target
initially reported success with zero tests; explicitly enabling its `guest`
feature corrected that harness, and its test log now confirms four executed tests.
Subsequent TCP/TLS timeout propagation changes passed the pkgd, lifecycle and
HTTPS targets again. Earlier affected runs also passed 141 kernel core tests and
eight timed tests, including privileged clock routing and RTC-only migration.

Host coverage includes sequential TUF updates, incremental ordered delegation
lookup, failed secure-state transitions, credential-generation isolation and
relative redirect scoping. CAS fault injection interrupts every write/sync
boundary of a replacement, including torn writes, then reopens each captured
image and requires a complete old or new transaction. Retained-store tests commit,
drop and reopen a real database without closing its externally owned storage.
Netstack tests cover partial writes, backpressure, EOF and cancelled connection
slot reuse. Lifecycle tests preserve caller/method/digest constraints, reject
missing inventory, exclude plaintext tokens and keys from checkpoints, and cancel
transport while retaining source recovery state. These do not establish physical
power-loss recovery or live guest replacement.

The HTTPS target contains four tests covering HTTP/2, HTTP/1.1, bearer challenges, mTLS,
signed OCI/TUF resolution, offline CAS retrieval and repeated 512 KiB transfers
across flow-control windows with partial I/O and injected backpressure, plus
timeout and unpolled-cancellation cleanup. Its fixture servers stop
and join on drop. The font fixture is parsed by the existing fontd implementation.

### Guest and build evidence

Pkgd's executable and replacement archive, appd, fontd and teed compiled for
both aarch64 and x86_64 earlier in implementation. The x86 guest probe also
compiled. Those x86 builds predate the latest networking and lifecycle changes;
they are not final-tree architecture acceptance. ARM acceptance builds and boots
with fresh source-built Trusty firmware containing the package-state endpoint.
Products extract source-built firmware through the existing `cached_firmware`
labels; refresh targets remain for saved recovery snapshots. Cold builds on this
host used libclang 21, bison, flex, m4 and dtc. No generated firmware is committed.

The maintained acceptance targets are
`//testing/e2e/qemu/pkg:package_resolution_test_{aarch64,x86_64}`. Their architecture
transition supplies `//testing/pkg_registry:guest_config.prototxt` through
`//services/pkgd:config_source`, leaving normal product trust empty. Bazel generates
certificates, signed metadata and artifact mappings for QEMU's local registry at
`10.0.2.2:18464`. The targets are exclusive because they share that configured
port. Signed fontd, timed and probe archives are preinstalled; the application,
driver and firmware archives are served through OCI. The firmware fixture is a
process-free library archive, not device-executable firmware.

ARM guests have passed KeyMint initialization, pkgd/fontd STORAGE startup,
validated RTC initialization, malformed-resource/cancellation/drop probes, and
sealed credential provisioning through Trusty. Integration fixes include:

- Appd retains one TeeManager binding for each channel-owned KeyMint/AVB session
  and dispatches the directory bindings collected during boot.
- Pkgd/fontd declare ServiceDirectory.Connect ordinal 2. Failed directory calls
  release owned endpoints; appd releases its temporary provider endpoint after
  delivery, including the duplicate on send failure.
- Timed revalidates RTC on boot. The test's prototxt disables SNTP, and kernel
  privileged dispatch now includes the SetTime capability.
- Netstack preserves bytes under stream backpressure, propagates EOF, reclaims
  cancelled connections and polls every client class without short-circuiting.
  Pkgd preserves TCP/TLS/HTTP timeout status for bounded retries.

The latest completed ARM run failed after 844.6 seconds. It completed bearer
authentication and requested digest-pinned metadata plus application, driver and
firmware payload URLs, but the consumer received `UNAVAILABLE` after a TCP setup
timeout. Requesting payload URLs is not proof of verified delivery. Its QEMU and
RPMB processes were verified stopped. A subsequent run with fair client polling,
typed timeouts and per-milestone diagnostics confirmed sealed provisioning and
HTTP payload timeouts/retries for application and driver requests. It was
deliberately stopped after 1124.1 seconds to correct the fixture's HTTP/2
connection shutdown; QEMU, RPMB and test processes were verified gone. Graceful
server shutdown, client completion ordering and a large-payload partial-I/O
regression passed all three HTTPS host tests. Another guest run is still required.

The next ARM run received 294,908 of the application's 361,234 bytes before the
fixed 30-second HTTP deadline expired. It was deliberately stopped after 867.4
seconds; its fixture processes were verified gone. The configuration now exposes
bounded TCP/TLS and HTTP deadlines. The signed test prototxt selects 30-second
setup and 180-second request limits, with a 240-second host connection limit and
300-second probe RPC limits. All five focused host targets passed: 21 pkgd tests,
four lifecycle tests, four HTTPS/deadline tests, five shared network tests and the
client dependency-policy check. Guest validation of the configured limits remains
pending; production defaults remain 10 and 30 seconds.

App installation, remote fonts, driver/firmware consumer acceptance, denied
writable mappings on resolved payloads, active-request credential replacement,
failed adoption, live replacement and reboot/offline persistence remain unpassed.
The fixture contains those assertions, but reaching their code is not validation.
No successful QEMU package-resolution result or physical-hardware result is claimed.

## Current gaps

### Implementation and test-harness gaps

These are source-inspection findings, separate from acceptance tests that have
not passed. They remain open in the working tree.

| Area | Current limitation | Work still required |
| --- | --- | --- |
| Resolver responsiveness | `transport.rs` synchronously binds Netstack through `ServiceDirectoryClient.connect`. `runtime.rs` also performs synchronous CAS and Trusty operations while servicing requests. Asynchronous network futures do not make these operations nonblocking. | Bound or make this work asynchronous and verify other clients, cancellation and lifecycle requests remain responsive during slow service/storage operations. |
| TCP buffer configuration | `lib/net/src/async_connect.rs` requests 64 KiB receive/transmit buffers, but netstack allocates fixed 8 KiB buffers. Its migration decoder also limits each buffer to 8 KiB. | Honor bounded socket options or reconcile the requested sizes with the supported contract, preserving migration compatibility. This mismatch alone has not been established as the cause of guest timeouts. |
| TLS trust-service resource ownership | `lib/net/src/secure.rs::fetch_tls_root_bundle` maps and copies a returned bundle but never closes its raw VMO handle, including error exits. Pkgd constructs a fresh root cache per job on this path. | Give the returned resource scoped ownership and test successful, malformed and failed replies. The registry fixture supplies explicit TLS roots and does not exercise this path. |
| Replacement during a pending read | The guest fixture checks that the pending-read success marker and transplant commit marker both exist, without requiring that success occurs after commit. Its registry hold automatically expires after 30 seconds. | Keep the request demonstrably pending through cutover and require post-commit completion; fail the test if the hold expires. The existing ordered check applies to retained consumer VMOs, not the pending read. |
| Credentials replaced during a request | The same expiring hold can release the request before provisioning completes; marker checks do not prove the required overlap and completion order. | Assert that the request remains pending during credential replacement, then verify completion with the intended credential generation and no reuse across generations. |

### Acceptance gaps

| Area | Evidence available | Evidence still missing |
| --- | --- | --- |
| Live OCI/TUF and HTTPS | Signed host fixtures pass HTTP/2, HTTP/1.1, bearer and mTLS tests. ARM reaches authenticated metadata and payload requests. | A verified artifact delivered in either guest architecture. The observed partial application transfer timed out at 294,908 of 361,234 bytes; the subsequent configurable deadlines have host coverage but no confirmed guest pass. |
| Apps and fonts | Appd/fontd host tests and source integration exist; the font fixture parses with the existing implementation. | Guest artifact installation and activation, remote font indexing and immutable VMO reuse, local-font priority and user isolation through the live resolver. |
| Drivers and firmware | Configured acquisition is routed through appd; the fixture checks OCI-origin installation records. | Passing guest acquisition and driver-policy/lifecycle checks. The process-free firmware archive does not establish device firmware execution or physical driver activation. |
| Protocol and security | Focused host tests cover TUF verification, policy, malformed data, credential generations and failed secure-state commits. ARM protocol/resource probes and sealed provisioning have passed. | The complete planned rejection matrix through real service boundaries, including protected-endpoint access, authorized cache hits, failed secure commits and denied writable mappings on successfully resolved payloads. |
| CAS and concurrency | Host tests exercise deduplication, cancellation, quotas, corruption and interrupted transactional writes; retained storage reopens successfully. | Guest offline retrieval and restart recovery, active transfer cancellation/coalescing under load, and storage fault behavior through durable STORAGE. Host fault injection does not establish physical power-loss recovery. |
| Heart transplant | Four pkgd lifecycle host tests cover inventory, constraints, private secret handles and source-state retention. Replacement archives exist. | Passing active-read replacement after strengthening the assertions above; surviving client bindings and VMOs; failed-adoption recovery with continued service; sealed credential and rollback persistence across reboot on both architectures. |
| Product and architecture integration | Earlier executable/replacement builds passed for both architectures; ARM acceptance builds and boots source-built Trusty. | Final-tree x86 builds after recent networking/lifecycle changes, x86 source-firmware and EFI boundary checks, and complete nongraphical/workstation assemblies for both architectures. |
| Regression coverage | The focused host results above passed. | The full affected networking, TUF, appd, fontd, teed, VFS, migration, assembly and architecture regression selection. Earlier or narrower passes do not establish a final-tree aggregate pass. |
| Hardware | QEMU and host evidence only. | Physical-hardware validation of secure storage, networking, driver/firmware activation and lifecycle persistence. |

Default products have no configured production trust roots, registries or
artifact mappings. This is an intentional provisioning boundary, not a test
failure; deployments must supply trusted prototxt configuration. Test-only
endpoints and keys must not be presented as production provisioning.

All guest acceptance attempts must clean up their registry, QEMU and RPMB
processes, including timeout and failure paths. No such processes remained at
this documentation checkpoint. There is no confirmed guest result for the latest
configured-deadline rerun, so it contributes no additional acceptance evidence.
