# RFC 0058: TUF metadata in OCI registries

- Created: 2026-09-07T10:33:31-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

OCI registries store TUF metadata alongside BexOS package payloads. Content-addressed blobs and role manifests provide delivery, while TUF verification governs freshness and trust.

## Design overview

Under the **OCI Distribution Spec** and **OCI Image Spec v1.1+ (Artifacts)**, an OCI registry provides authenticated, content-addressable storage (CAS). Standard OCI endpoints can serve the entire TUF metadata lifecycle alongside `.bex` payloads.

Storing TUF metadata in the registry removes the need for a separate metadata HTTP server while preserving TUF’s end-to-end security guarantees.

## The Fundamental Mapping: How TUF Maps to OCI

TUF defines two classes of files with distinct lifecycles:

### Immutable, Content-Addressable Targets (`targets.json` and `.bex` packages)

* These map directly to **OCI Blobs** (`/v2/<name>/blobs/sha256:<digest>`).

### Mutable, Fast-Expiring Metadata (`timestamp.json`, `snapshot.json`, `root.json`)

* These map to **OCI Manifests** referenced via dedicated, mutable tags (e.g., `:tuf-timestamp`, `:tuf-snapshot`, `:tuf-root`).

## Implementation Architecture: TUF Metadata as OCI Artifacts

TUF metadata can use a reserved repository namespace (e.g., `pkg.bexos.org/sys/tuf` or scoped to the package repository itself under `<repo>/_tuf`):

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ OCI REGISTRY (Docker Hub, Harbor, GHCR, AWS ECR, Azure ACR)                 │
│                                                                             │
│  Namespace: `bexos/tuf`                                                     │
│                                                                             │
│  [ Tag: :tuf-timestamp ] ──► OCI Manifest ──► Blob: `timestamp.json`       │
│                                      │                                      │
│  [ Tag: :tuf-snapshot ]  ──► OCI Manifest ──► Blob: `snapshot.json`        │
│                                      │                                      │
│  [ Tag: :tuf-targets ]   ──► OCI Manifest ──► Blob: `targets.json`         │
│                                      │                                      │
│  [ Tag: :tuf-root ]      ──► OCI Manifest ──► Blob: `root.json`            │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │
                                       │ Standard OCI Distribution API
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ BEXOS CLIENT (`pkgd` / `tuf_client`)                                        │
│                                                                             │
│  1. `GET /v2/bexos/tuf/manifests/tuf-timestamp`                             │
│  2. Download & parse `timestamp.json` blob                                  │
│  3. Verify signature, check anti-rollback against RPMB                      │
│  4. Read immutable snapshot digest from `timestamp.json`                     │
│  5. Fetch `snapshot.json` blob directly by SHA256                           │
│  6. Fetch `targets.json` blob directly by SHA256                            │
│  7. Fetch `.bex` payload blob directly by SHA256                            │
└─────────────────────────────────────────────────────────────────────────────┘

```

### The OCI Manifest Format for a TUF Role

Each TUF role file is pushed as an OCI artifact manifest:

```json
{
  "schemaVersion": 2,
  "mediaType": "application/vnd.oci.image.manifest.v1+json",
  "artifactType": "application/vnd.bexos.tuf.role.v1",
  "config": {
    "mediaType": "application/vnd.oci.empty.v1+json",
    "data": "{}"
  },
  "layers": [
    {
      "mediaType": "application/vnd.bexos.tuf.timestamp.v1+json",
      "digest": "sha256:d48e89451478546b5a...",
      "size": 612
    }
  ],
  "annotations": {
    "bexos.org/tuf-version": "14",
    "bexos.org/tuf-expires": "2026-09-08T12:00:00Z"
  }
}

```

## How the Client Validates via OCI Endpoints

The verification sequence adheres strictly to the TUF standard client workflow, using standard OCI Distribution endpoints:

### Step 1: Check Freshness via `timestamp` Manifest

* Client calls: `GET /v2/bexos/tuf/manifests/tuf-timestamp`
* It extracts the single layer blob digest (`sha256:d48e894...`) and pulls the blob via `GET /v2/bexos/tuf/blobs/sha256:d48e894...`.
* The client parses `timestamp.json` and verifies its signature against trusted keys.
* *Anti-Rollback Check:* It compares `timestamp.json`'s version against the device's monotonic version stored in RPMB/Trusty.

### Step 2: Pull `snapshot.json` by Content Digest

* `timestamp.json` specifies the exact hash for `snapshot.json` (e.g., `sha256:a1b2c3...`).
* The client skips mutable tags and queries the blob directly: `GET /v2/bexos/tuf/blobs/sha256:a1b2c3...`.
* Verifies the snapshot signature and checks that snapshot role versions are monotonic.

### Step 3: Pull `targets.json` by Content Digest

* `snapshot.json` specifies the exact hash for `targets.json` (e.g., `sha256:f4e3d2...`).
* The client queries: `GET /v2/bexos/tuf/blobs/sha256:f4e3d2...`.

### Step 4: Pull the App or Driver Payload

* `targets.json` maps `bexos.calc:1.2.0` to layer hash `sha256:8892ac...`.
* The client streams the `.bex` package directly: `GET /v2/bexos/apps/calc/blobs/sha256:8892ac...`.

## Handling the OCI Mutability Risk

The primary security challenge of OCI registries is that **tags are mutable**: a compromised registry operator could silently overwrite `:tuf-timestamp` with an older manifest or point it to junk.

TUF completely eliminates this risk:

* **Anti-Freeze / Anti-Replay:** If a rogue registry admin points `:tuf-timestamp` to a valid, signed manifest from last month, the client's TUF engine rejects it because the expiration timestamp has lapsed, or because its version counter is lower than the counter recorded in local storage/Trusty RPMB.
* **Malicious Tampering:** If the registry admin alters the payload inside the layer blob, signature verification against the offline root of trust fails immediately.
* **Tag Downgrade:** Because only the initial discovery step uses a mutable tag (`:tuf-timestamp`), and all subsequent hops (`snapshot` -> `targets` -> `.bex` binary) are fetched strictly via immutable content digests asserted inside signed TUF documents, tag manipulation cannot trick the client into running untrusted code.

## Advantages for BexOS Infrastructure

* **Zero Custom Update Servers:** CI/CD pipelines can publish updates by running standard OCI tools (`oras push`, `crane`, or a custom Rust CLI) straight to Docker Hub, GitHub Packages, or enterprise Harbor instances.
* **Works with Enterprise Air-Gaps:** Enterprises mirroring container images via tools like `skopeo copy` or `oras copy` automatically mirror both the application payloads and the TUF metadata trees into their offline enclaves without custom scraping scripts.
* **Unified Auth & RBAC:** Device fleet tokens use standard OCI Bearer Token auth (`/v2/token?service=registry&scope=repository:bexos/tuf:pull`) to control access to proprietary or internal enterprise driver/app updates.
