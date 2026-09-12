# RFC-0064: Centralized Package Daemon (`pkgd`), Secure OCI/TUF Resolution, and Client Stub Architecture

* **Author:** BexOS Security & Package Architecture Working Group
* **Status:** Proposed
* **Target Subsystems:** `pkgd`, `libpkg_client`, `appd`, `devhost`, `fontd`, `networkd`, `trusty`
* **Applicability:** Package, Driver, and Font Lifecycle Management

---

## 1. Summary

This RFC specifies the unified delivery architecture for all software artifacts on BexOS (applications, drivers, and fonts). It introduces:

1. **`pkgd` (Package Daemon):** A D1 system service that acts as the sole custodian of network credentials, OCI distribution interactions, anti-rollback validation via TUF, and on-disk content-addressable storage (CAS).
2. **`libpkg_client`:** A non-networked, capability-gated Rust client stub library that wraps the IPC interface to `pkgd`.
3. **Zero-Copy Handoff:** Distribution of verified immutable package payloads to consumer services (`appd`, `devhost`, `fontd`) via read-only Virtual Memory Objects (`zx.Handle:VMO`).

---

## 2. Motivation

BexOS uses OCI registries with signed TUF metadata to distribute applications, peripheral drivers, and dynamic fonts. Historically, operating systems either implement client-side package retrieval directly within application runtimes or rely on monolithic root daemons that perform unauthenticated local execution.

Distributing fetching and credential handling into a client-side shared library creates unacceptable security risks:

* **Capability Creep:** Consumer services like `fontd` or hardware driver hosts (`devhost`) would require raw network sockets (`bexos.net.SocketProvider`). A memory safety vulnerability in a font parser or hardware driver would immediately yield unattenuated remote network egress.
* **Credential Proliferation:** Enterprise registry tokens, OAuth2 bearer tokens, and corporate mTLS client certificates would need to be mapped into the virtual address spaces of unprivileged worker processes.
* **Race Conditions & Anti-Rollback Desynchronization:** TUF client verification demands strict adherence to monotonic version updates stored in non-volatile secure storage (Trusty/RPMB). Concurrent execution across client libraries risks race conditions during version progression.

Consolidating network operations, credential isolation, and cryptographic integrity checks into `pkgd` ensures strict adherence to the principle of least privilege while providing system-wide deduplication.

---

## 3. Detailed Design

```
┌──────────────────────┐   ┌──────────────────────┐   ┌──────────────────────┐
│  `devhost` (Drivers) │   │   `appd` (User Apps) │   │    `fontd` (Fonts)   │
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
│ `networkd` ──► OCI Registries (Docker Hub, Harbor, Cloud ACR/ECR/GAR)      │
└────────────────────────────────────────────────────────────────────────────┘

```

---

### 3.1 Division of Responsibilities

#### The Dedicated Service (`pkgd`)

* **Network Execution:** `pkgd` is the only package-related service granted outbound network access via `bexos.net.SocketProvider`.
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
* Exposes an asynchronous Rust API returning strongly typed handles (`zx::Vmo`).

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
      │ 8. Return `zx.Handle:VMO`  │                                                │
      │◄───────────────────────────┤                                                │

```

1. **Resolution Request:** Consumer invokes `ResolveArtifact` over FIDL, passing an artifact URI (e.g., `pkg.bexos.org/fonts/fira-code:latest`) or a direct target content digest.
2. **Local Cache Evaluation:** `pkgd` checks its local CAS. If the content hash is valid and uncorrupted, `pkgd` duplicates an existing read-only `VMO` handle and returns immediately.
3. **TUF Freshness Verification:** On a cache miss, `pkgd` pulls the `:tuf-timestamp` OCI manifest from the remote registry.
4. **Hardware Anti-Rollback Check:** `pkgd` queries `trusty` over TIPC to ensure the timestamp version is greater than or equal to the device's monotonic version stored in RPMB.
5. **Payload Fetching:** `pkgd` streams target blobs by exact cryptographic hash directly into a kernel VMO.
6. **Integrity Validation:** The complete buffer's SHA-256 digest is validated against the signed `targets.json` record.
7. **Capability Handoff:** `pkgd` strips write permissions using `zx_handle_replace(..., ZX_RIGHT_READ | ZX_RIGHT_MAP)` and sends the read-only VMO handle to the client.

---

## 4. Interface Definition Language (FIDL)

The protocol boundary lives in `idl/bexos/pkg/resolver.fidl`:

```fidl
library bexos.pkg;

using bexos.kernel;

type HashType : uint8 {
    SHA256 = 1;
    BLAKE3 = 2;
};

struct BlobDigest {
    type HashType;
    digest array<uint8, 32>;
};

type ArtifactKind : uint8 {
    APPLICATION = 1;
    DRIVER = 2;
    FONT = 3;
    FIRMWARE = 4;
};

struct ArtifactQuery {
    registry_host string:128;   // e.g. "pkg.bexos.org"
    repository string:128;      // e.g. "fonts/fira-code"
    tag string:64;              // e.g. "latest", "v1.2.0"
    expected_digest box<BlobDigest>;
    kind ArtifactKind;
};

@discoverable
protocol PackageResolver {
    /// Resolves an artifact query to an immutable, verified read-only memory object
    ResolveArtifact(struct {
        query ArtifactQuery;
    }) -> (resource struct {
        data zx.Handle:VMO;
        content_length uint64;
        verified_digest BlobDigest;
    }) error bexos.kernel.Status;

    /// Fetches a raw content-addressed blob directly by hash
    ResolveBlob(struct {
        digest BlobDigest;
    }) -> (resource struct {
        data zx.Handle:VMO;
        content_length uint64;
    }) error bexos.kernel.Status;
};

@discoverable
protocol CredentialManager {
    /// Privileged method exposed strictly to enterprise MDM / SysUI
    SetRegistryCredential(struct {
        registry_host string:128;
        auth_token string:1024;
        sealed_in_trusty bool;
    }) -> () error bexos.kernel.Status;
};

```

---

## 5. Client Library Integration (`libpkg_client`)

Applications and platform services do not interact with raw FIDL handles directly; they consume the ergonomic `libpkg_client` Rust library:

```rust
pub struct PackageClient {
    proxy: PackageResolverProxy,
}

impl PackageClient {
    pub fn connect() -> Result<Self, PackageError> {
        let proxy = bexos_component::client::connect_to_protocol::<PackageResolverMarker>()
            .map_err(|_| PackageError::ServiceUnavailable)?;
        Ok(Self { proxy })
    }

    pub async fn fetch_artifact(
        &self,
        registry: &str,
        repository: &str,
        tag: &str,
        kind: ArtifactKind,
    ) -> Result<zx::Vmo, PackageError> {
        let query = ArtifactQuery {
            registry_host: registry.to_string(),
            repository: repository.to_string(),
            tag: tag.to_string(),
            expected_digest: None,
            kind,
        };

        let response = self.proxy.resolve_artifact(&query).await
            .map_err(|e| PackageError::IpcFailure(e))?
            .map_err(|status| PackageError::ResolutionFailed(status))?;

        Ok(response.data)
    }
}

```

---

## 6. Security Analysis & Threat Modeling

| Threat | Mitigation Mechanism |
| --- | --- |
| **Compromised Consumer Service (e.g., parser zero-day in `fontd`)** | `fontd` has no network socket capability. The exploited process cannot open sockets, initiate outbound leaks, or communicate with rogue C2 servers. |
| **Malicious Registry Operator / Man-in-the-Middle (MitM)** | Tag mutation or corrupted blobs are rejected because payload hashes are cross-checked against offline-signed TUF metadata. |
| **Freeze / Replay / Rollback Attacks** | The client compares `timestamp.json` version numbers against local monotonic hardware storage managed by `trusty` via RPMB. Expired or downgraded metadata is rejected. |
| **Credential Theft from Disk** | Enterprise registry tokens are held exclusively in volatile memory inside `pkgd` or sealed with hardware-backed encryption via `trusty`. They are never readable by user sandboxes. |
| **Shared Memory Tampering** | Payloads are transferred as read-only VMOs (`ZX_RIGHT_READ |

---

## 7. Implementation Roadmap

### Phase 1: `pkgd` Local CAS & Core Daemon

* Stand up the `pkgd` D1 component scaffold using Bazel.
* Implement the local Content-Addressed Storage engine backed by the system cache partition.
* Implement `PackageResolver.ResolveBlob` serving static pre-cached assets as read-only VMOs.

### Phase 2: OCI Network Engine & `libpkg_client`

* Integrate the asynchronous HTTP/2 client into `pkgd` with `networkd` socket capabilities.
* Implement `libpkg_client` and migrate `fontd` and `appd` to use it.
* Validate request coalescing across concurrent identical artifact requests.

### Phase 3: TUF Verification & Hardware Rollback Protection

* Implement the TUF client verification pipeline inside `pkgd`.
* Hook version assertion into `trusty` over TIPC to track non-volatile monotonic counters.
* Implement enterprise credential storage with `CredentialManager` gated by privileged capabilities.