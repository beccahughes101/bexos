# RFC-0064: Centralized Package Daemon (`pkgd`), Secure OCI/TUF Resolution, and Client Stub Architecture

* **Author:** BexOS Security & Package Architecture Working Group
* **Status:** Implementation in progress; acceptance tracked in [CURRENT.md](CURRENT.md)
* **Target Subsystems:** `pkgd`, `libpkg_client`, `appd`, `appd` driver management, `fontd`, `netstack`, `trusty`
* **Applicability:** Package, Driver, and Font Lifecycle Management

---

## 1. Summary

This RFC specifies the unified delivery architecture for all software artifacts on BexOS (applications, drivers, and fonts). It introduces:

1. **`pkgd` (Package Daemon):** A D1 system service that acts as the sole custodian of network credentials, OCI distribution interactions, anti-rollback validation via TUF, and on-disk content-addressable storage (CAS).
2. **`libpkg_client`:** A non-networked, capability-gated Rust client stub library that wraps the IPC interface to `pkgd`.
3. **Zero-Copy Handoff:** Distribution of verified immutable package payloads to consumer services (`appd`, `appd` driver management, `fontd`) via read-only Virtual Memory Objects (`handle:VMO`).

---

## 2. Motivation

BexOS uses OCI registries with signed TUF metadata to distribute applications, peripheral drivers, and dynamic fonts. Historically, operating systems either implement client-side package retrieval directly within application runtimes or rely on monolithic root daemons that perform unauthenticated local execution.

Distributing fetching and credential handling into a client-side shared library creates unacceptable security risks:

* **Capability Creep:** Consumer services like `fontd` or hardware driver hosts (`appd` driver management) would require raw network sockets (`bexos.net.Netstack`). A memory safety vulnerability in a font parser or hardware driver would immediately yield unattenuated remote network egress.
* **Credential Proliferation:** Enterprise registry tokens, OAuth2 bearer tokens, and corporate mTLS client certificates would need to be mapped into the virtual address spaces of unprivileged worker processes.
* **Race Conditions & Anti-Rollback Desynchronization:** TUF client verification demands strict adherence to monotonic version updates stored in non-volatile secure storage (Trusty/RPMB). Concurrent execution across client libraries risks race conditions during version progression.

Consolidating network operations, credential isolation, and cryptographic integrity checks into `pkgd` ensures strict adherence to the principle of least privilege while providing system-wide deduplication.

---

## 3. Detailed Design

```
┌──────────────────────┐   ┌──────────────────────┐   ┌──────────────────────┐
│  appd (Drivers)     │   │   appd (User Apps)  │   │    fontd (Fonts)    │
│  [libpkg_client]     │   │   [libpkg_client]    │   │    [libpkg_client]   │
│  (No Net Capability) │   │   (No Net Capability)│   │    (No Net Capability)│
└──────────┬───────────┘   └──────────┬───────────┘   └──────────┬───────────┘
           │                          │                          │
           │ FIDL: PackageResolver.ResolveArtifact(...)          │
           └──────────────────────────┼──────────────────────────┘
                                      ▼
┌────────────────────────────────────────────────────────────────────────────┐
│ `pkgd` (D1 Package Management Daemon)                                      │
│                                                                            │
│  ├── [ Credential Vault ] ◄── Trusty TIPC (Sealed Keystore Tokens)         │
│  │                                                                         │
│  ├── [ TUF Verification Engine ] ◄── Monotonic Rollback Counter (RPMB)     │
│  │                                                                         │
│  ├── [ In-Flight Request Deduplication Table ]                             │
│  │                                                                         │
│  └── [ Local CAS (Content-Addressed Storage) ]                             │
│      • In-memory block cache & verified disk blocks                        │
└─────────────────────────────────────┬──────────────────────────────────────┘
                                      │ Outbound Network FIDL
                                      ▼
┌────────────────────────────────────────────────────────────────────────────┐
│ `netstack` ──► OCI Registries (Docker Hub, Harbor, Cloud ACR/ECR/GAR)      │
└────────────────────────────────────────────────────────────────────────────┘

```

---

### 3.1 Division of Responsibilities

#### The Dedicated Service (`pkgd`)

* **Network Execution:** `pkgd` is the only package-related service granted outbound network access via `bexos.net.Netstack`.
* **Credential Lifecycle:** Secures registry tokens, enterprise bearer tokens, and client mTLS keys. Secrets are stored in private address space memory and never mirrored over IPC or persisted in unencrypted userspace storage.
* **TUF Client Verification:** Implements the complete TUF state machine:
* Pulls mutable `:tuf-timestamp` manifests.
* Verifies signatures against root keys and validates monotonic version counters against hardware storage via `trusty`.
* Fetches and verifies immutable `snapshot.json`, `targets.json`, and target payload blobs.


* **Request Coalescing:** Identical requests for the same content hash or artifact tag are merged into a single in-flight OCI blob transfer.
* **Content-Addressed Storage (CAS):** Stores verified layer blobs on disk keyed by SHA-256 digests.

#### The Client Stub (`libpkg_client`)

* Contains zero network drivers, zero TLS stacks, and zero credential handlers.
* Manages the lifecycle of the client-side FIDL channel to `pkgd`.
* Exposes an asynchronous Rust API returning strongly typed handles (`ReadOnlyVmo`).

---

### 3.2 Dynamic Resolution & Ingestion Pipeline

```
Client (`fontd`)                `pkgd`                  `trusty`              OCI Registry
      │                            │                        │                       │
      │ 1. ResolveArtifact(Query)  │                        │                       │
      ├───────────────────────────►│                        │                       │
      │                            │ 2. Check CAS Cache     │                       │
      │                            │    (Cache Miss)        │                       │
      │                            │                        │                       │
      │                            │ 3. Fetch :tuf-timestamp                        │
      │                            ├───────────────────────────────────────────────►│
      │                            │◄───────────────────────────────────────────────┤
      │                            │                                                │
      │                            │ 4. Verify Monotonic Counter                    │
      │                            ├───────────────────────►│                       │
      │                            │◄───────────────────────┤                       │
      │                            │                                                │
      │                            │ 5. Pull & Validate Snapshot/Targets/Payload    │
      │                            ├───────────────────────────────────────────────►│
      │                            │◄───────────────────────────────────────────────┤
      │                            │                                                │
      │                            │ 6. Commit to local CAS                         │
      │                            │ 7. Allocate Read-Only VMO                      │
      │ 8. Return `handle:VMO`  │                                                │
      │◄───────────────────────────┤                                                │

```

1. **Resolution Request:** Consumer invokes `ResolveArtifact` over FIDL, passing an artifact URI (e.g., `pkg.bexos.org/fonts/fira-code:latest`) or a direct target content digest.
2. **Local Cache Evaluation:** Digest queries may use rehashed cached bytes only when persisted provenance authorizes the caller and artifact kind. Named queries always validate metadata freshness, including on cache hits.
3. **TUF Freshness Verification:** For every named query, `pkgd` pulls the `:tuf-timestamp` OCI manifest from the remote registry.
4. **Hardware Anti-Rollback Check:** `pkgd` queries `trusty` over TIPC to ensure the timestamp version is greater than or equal to the device's monotonic version stored in RPMB.
5. **Payload Fetching:** `pkgd` streams target blobs by exact cryptographic hash directly into a kernel VMO.
6. **Integrity Validation:** The complete buffer's SHA-256 digest is validated against the signed `targets.json` record.
7. **Capability Handoff:** After verification, pkgd removes its writable mapping and closes the writable staging handle. It retains an immutable canonical handle with duplication rights and sends clients only `TRANSFER | READ | MAP`. Secure rollback state must commit before content is released.

---

## 4. Interface Definition Language (FIDL)

The protocol boundary lives in `idl/bexos/pkg/resolver.fidl`:

```fidl
library bexos.pkg;

type PackageStatus = strict enum : int32 {
    OK = 0;
    NOT_FOUND = -10;
    ACCESS_DENIED = -2;
    INVALID_ARGS = -8;
    RESOURCE_EXHAUSTED = -9;
    VERIFY_FAILED = -21;
    UNAVAILABLE = -95;
    IO = -14;
    TIMED_OUT = -6;
};
type HashType = strict enum : uint8 { SHA256 = 1; BLAKE3 = 2; };
type ArtifactKind = strict enum : uint8 { APPLICATION = 1; DRIVER = 2; FONT = 3; FIRMWARE = 4; };
struct BlobDigest { hash_type HashType; digest array<uint8, 32>; };
struct ArtifactQuery {
    registry_host string:128;
    repository string:128;
    tag string:64;
    expected_digest vector<BlobDigest>:1;
    kind ArtifactKind;
};
resource struct ResolvedBlob {
    data handle:VMO;
    content_length uint64;
    verified_digest BlobDigest;
};
@discoverable
protocol PackageResolver {
    1: ResolveArtifact(struct { query ArtifactQuery; }) -> (resource struct {
        status PackageStatus;
        blob vector<ResolvedBlob>:1;
    });
    2: ResolveBlob(struct { digest BlobDigest; allow_network_fetch bool; }) -> (resource struct {
        status PackageStatus;
        blob vector<ResolvedBlob>:1;
    });
};
@discoverable
protocol CredentialManager {
    1: SetRegistryCredential(struct {
        registry_host string:128;
        auth_token string:1024;
        sealed_in_trusty bool;
    }) -> (struct { status PackageStatus; });
    2: SetRegistryMtlsIdentity(resource struct {
        registry_host string:128;
        certificate_chain handle:VMO;
        certificate_chain_length uint64;
        private_key handle:VMO;
        private_key_length uint64;
        sealed_in_trusty bool;
    }) -> (struct { status PackageStatus; });
    3: RemoveRegistryCredential(struct { registry_host string:128; }) -> (struct { status PackageStatus; });
};

```

---

## 5. Client Library Integration (`libpkg_client`)

Applications and platform services do not interact with raw FIDL handles directly; they consume the ergonomic `libpkg_client` Rust library:

```rust
use bexos_pkg_client::{ArtifactKind, ArtifactQuery, PackageClient};

// `endpoint` is a capability-scoped BexOS Channel provided by service routing.
let mut client = PackageClient::from_channel(endpoint);
let query = ArtifactQuery {
    registry_host: "registry.example".into(),
    repository: "fonts/example".into(),
    tag: "latest".into(),
    expected_digest: None,
    kind: ArtifactKind::Font,
};
let font = client.fetch_artifact(&query).await?;
// font owns the verified read-only mapping and closes it on drop.
consume_font(font.bytes());
```

---

## 6. Security Analysis & Threat Modeling

| Threat | Mitigation Mechanism |
| --- | --- |
| **Compromised Consumer Service (e.g., parser zero-day in `fontd`)** | `fontd` has no network socket capability. The exploited process cannot open sockets, initiate outbound leaks, or communicate with rogue C2 servers. |
| **Malicious Registry Operator / Man-in-the-Middle (MitM)** | Tag mutation or corrupted blobs are rejected because payload hashes are cross-checked against offline-signed TUF metadata. |
| **Freeze / Replay / Rollback Attacks** | The client compares `timestamp.json` version numbers against local monotonic hardware storage managed by `trusty` via RPMB. Expired or downgraded metadata is rejected. |
| **Credential Theft from Disk** | Enterprise registry tokens are held exclusively in volatile memory inside `pkgd` or sealed with hardware-backed encryption via `trusty`. They are never readable by user sandboxes. |
| **Shared Memory Tampering** | Payloads are transferred with `TRANSFER`, `READ`, and `MAP` rights only; consumers cannot duplicate or map them writable. |

---

## 7. Implementation Roadmap

### Phase 1: `pkgd` Local CAS & Core Daemon

* Stand up the `pkgd` D1 component scaffold using Bazel.
* Implement the local Content-Addressed Storage engine backed by the system cache partition.
* Implement `PackageResolver.ResolveBlob` serving static pre-cached assets as read-only VMOs.

### Phase 2: OCI Network Engine & `libpkg_client`

* Integrate the asynchronous HTTP/2 client into `pkgd` with `netstack` socket capabilities.
* Implement `libpkg_client` and migrate `fontd` and `appd` to use it.
* Validate request coalescing across concurrent identical artifact requests.

### Phase 3: TUF Verification & Hardware Rollback Protection

* Implement the TUF client verification pipeline inside `pkgd`.
* Hook version assertion into `trusty` over TIPC to track non-volatile monotonic counters.
* Implement enterprise credential storage with `CredentialManager` gated by privileged capabilities.

## 8. Repository implementation mapping

The long-term requirements above remain the acceptance target. See
[CURRENT.md](CURRENT.md) for verified behavior and
[current gaps](CURRENT.md#current-gaps). Source exists across all three phases,
but no phase has complete acceptance evidence. Open implementation work includes
synchronous service/storage calls, TCP buffer configuration and TLS root-handle
cleanup; lifecycle fixture ordering also needs strengthening. No guest run has
yet demonstrated verified artifact delivery. These limitations do not remove
requirements from the design above.

| Concern | Implementation |
| --- | --- |
| Wire API and product config | `idl/bexos/pkg/resolver.fidl`, `config.proto`; Bazel generates bindings and encodes prototxt |
| Owning asynchronous client | `lib/pkg_client` |
| Resolver, CAS, credential and migration modules | `services/pkgd/src` |
| HTTPS streaming over BexOS sockets | `lib/net/src/async_http.rs`, `http_stream.rs` |
| TUF signatures, root rotation and delegation authorization | `lib/tuf`; incremental target traversal in `src/search.rs`, path patterns in `src/pattern.rs` |
| Durable database ownership during replacement | `lib/redb/src/retained.rs`, pkgd's migration resources |
| Protected rollback and credential records | `secure/orchestrator/trusty/package_storage.c`, teed's internal PackageState capability |
| App installation | `services/appd/src/package_install.rs`, AppManager ordinal 4 |
| Font misses | `services/fontd/src/remote.rs` |

Products supply offline trusted roots, repository IDs, consumer kind grants,
origin restrictions and artifact mappings in `services/pkgd/package/config.prototxt`.
The checked-in default intentionally contains no production registry or trust root.
Network deadlines are also supplied through prototxt:

```protobuf
connect_timeout_ms: 10000
request_timeout_ms: 30000
```

These are the defaults for each TCP-setup/TLS phase and HTTP request. Zero is
invalid; configuration caps them at 120,000 and 600,000 milliseconds respectively.
The acceptance fixture uses longer bounded deadlines for ARM emulation. All
configuration encoding remains in Bazel.

Named tags select signed target paths; `custom.bexos.kind` must be `application`,
`driver`, `font`, or `firmware`. This package vocabulary is separate from the
runtime updater's existing artifact-kind vocabulary. Sequential root manifests
use `tuf-root-N`; timestamp discovery uses `tuf-timestamp`. Snapshot and targets
metadata are fetched through authenticated SHA-256 OCI blob descriptors.
Pkgd follows only delegations matching the requested signed target path, commits
each verified role before continuing, and bounds the traversal and cumulative
metadata download. Its incremental lookup preserves delegation order, terminating
roles and visited-role detection.

The maintained signed local registry fixture lives in `testing/pkg_registry`,
with guest acceptance in `testing/e2e/qemu/pkg`. Its Bazel architecture transition
selects test-only trust through `//services/pkgd:config_source`; normal product
configuration remains empty until roots, repository origins and mappings are
provisioned. Product firmware is built from source so the protected Trusty
package-state endpoint is included. Saved firmware refresh targets remain
available for explicit recovery snapshots. See `CURRENT.md` for actual test
results and the remaining acceptance gates.
