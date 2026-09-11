# RFC 0035: Application signing and trust stores

- Created: 2026-08-30T14:46:10-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

Trustd separates application-signing roots from TLS roots and provides capability-scoped trust validation. Package identity constraints, revocation, and update policy preserve that separation.

## Design overview

Current implementation status: BexOS builds selected dev/prod ecosystem
profiles under `//ecosystem/bexos`, embeds the selected public bundle at
`/system/ecosystem/bexos.bundle`, and still places generated TLS and
app-signing redb stores in BootFS for early trustd startup. `trustd` is a wave 3
BootFS service exposing algorithm/tier-aware `AppTrustManager` validation and a
read-only `TlsTrustManager` surface for TLS root bundle export. The deployed
service is a std-linked Tokio daemon with implemented heart-transplant state.
It validates Ed25519 and ECDSA P-256 package signatures, builds ordered DER
X.509 leaf/intermediate/root paths, verifies issuer signatures and self-signed
anchors, enforces CA/basic-constraints, key-usage, code-signing EKU, validity,
prefix, tier, anchor-selection, and revocation policy across immutable and
dynamic roots, applies monotonic revocation payloads, supports privileged
enterprise-root install/list/remove, enforces method filtering, and preserves
both root stores, dynamic trust state, and live client channels across
transplant.

Build the development stores with:

```sh
bazel build //ecosystem/bexos:public_bundle //ecosystem/bexos/dev:profile //ecosystem/bexos/prod:profile
```

App code-signing trust anchors must be separated from Web PKI TLS certificates. Web PKI has hundreds of public Certificate Authorities (CAs) intended for HTTPS domain validation, whereas App Code Signing requires a small, tightly controlled set of hardware-anchored roots of trust.

The following sections define how `trustd` manages app-signing root bundles and how `appd` consumes them.

## Dual-Keystore Architecture in `trustd`

`trustd` manages two distinct, isolated trust bundles in `/system/certs/`:

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ trustd Root Store Management                                                │
├─────────────────────────────────────────────────────────────────────────────┤
│ 1. TLS / Web PKI Bundle (`/system/certs/tls_roots.redb` & shared VMO)        │
│    • Hundreds of Web CAs (ISRG Root X1, DigiCert, GlobalSign, etc.)         │
│    • Consumed by: in-process TLS libraries (rustls/webpki), netstack DoH    │
│                                                                             │
│ 2. App Code Signing Bundle (`/system/certs/app_signing_roots.redb`)         │
│    • Minimal, hardware-anchored Ed25519/ECDSA root keys                     │
│    • Tier-scoped trust anchors (Platform, Enterprise MDM, Verified Stores)  │
│    • Consumed strictly by: `appd` and system update verification pipelines  │
└─────────────────────────────────────────────────────────────────────────────┘

```

## App Signing Trust Schema (`trust.proto`)

App signing trust anchors carry tier levels, allowed package prefix constraints, and revocation parameters:

```protobuf
syntax = "proto3";

package bexos.security.trust;

enum TrustTier {
  TIER0_BASE_SYSTEM  = 0; // OS components, core drivers, microkernel patches
  TIER1_PLATFORM_APP = 1; // First-party system apps, settings, system UI
  TIER2_VERIFIED_ECO = 2; // Verified stores (BexOS Store, Amazon, Samsung)
  TIER3_ENTERPRISE   = 3; // MDM-injected corporate developer keys
  TIER4_WEB_ORIGIN   = 4; // Well-known verified web domains (e.g. monzo.com)
}

message AppSigningRootAnchor {
  string anchor_id = 1;            // e.g. "bexos-root-prod-2026"
  TrustTier tier = 2;
  string algorithm = 3;            // "Ed25519" or "ECDSA-P256"
  bytes public_key_bytes = 4;

  // Namespace constraints (e.g. ["com.bexos.*"] or ["*"])
  repeated string permitted_package_prefixes = 5;

  uint64 valid_from = 6;
  uint64 valid_until = 7;
  bool is_hardware_anchored = 8;   // Pinned in ROM/OTP/RPMB
}

```

## Root-of-Trust Anchoring & Hierarchy

App signing roots follow the device authority hierarchy (`Platform > Enterprise/MDM > Store`):

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ 1. Hardware Root of Trust (OTP / RPMB / Secure Boot ROM)                    │
│    • Holds BexOS Core Public Key Hash (ROTPK)                               │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ Authenticates
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ 2. Immutable Platform Root Bundle (`/system/certs/platform_roots.bin`)      │
│    • Root keys for Tier 0 (Core OS) and Tier 1 (System Apps)                │
│    • Can only be updated via A/B whole-system cryptographic updates         │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ Manages runtime roots
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ 3. Dynamic Store & Enterprise Roots (`/system/certs/app_signing_roots.redb`)│
│    • Tier 2: Partner App Stores (Amazon, Samsung) signed by Platform Root   │
│    • Tier 3: Enterprise MDM keys injected during enrollment                 │
│    • Tier 4: Cached `.well-known` web developer fingerprints                │
└─────────────────────────────────────────────────────────────────────────────┘

```

## Signature Verification Flow in `appd`

When an app (`.bex`) is installed from the web, an app store, or an MDM payload:

```
[ Package (.bex) Ingestion ]
             │
             ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ 1. Extract Certificate Chain & Package Manifest                             │
│    • Extract package signer public key, signature, and intermediate certs   │
└────────────────────────────┬────────────────────────────────────────────────┘
                             │
                             ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ 2. Query `trustd.ValidateAppSigner()` via FIDL                              │
│    • `trustd` checks if the chain terminates in a trusted anchor            │
│    • `trustd` checks TUF/RPMB anti-rollback & revocation blocklists         │
│    • `trustd` validates prefix constraints (e.g. non-Bex key cannot sign    │
│      `com.bexos.*`)                                                         │
└────────────────────────────┬────────────────────────────────────────────────┘
                             │
                             ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ 3. Grant Tier & Sandbox Boundary                                            │
│    • Tier 0/1: Granted system capability channels                           │
│    • Tier 2/3/4: Restricted to unprivileged user-prompted sandbox          │
└─────────────────────────────────────────────────────────────────────────────┘

```

## `bexos.security.trust` FIDL Protocol

`trustd` exposes dedicated methods for app signature validation without exposing the mutating root APIs to unprivileged apps:

```fidl
library bexos.security.trust;

using bexos.kernel;

type VerificationStatus = strict enum : uint8 {
    VALID                 = 1;
    UNTRUSTED_ROOT        = 2;
    PREFIX_VIOLATION      = 3; // Key not authorized for package prefix
    EXPIRED_CERTIFICATE   = 4;
    REVOKED_CERTIFICATE   = 5;
    INSUFFICIENT_TIER     = 6; // Attempted to sign system driver with store key
};

type AppValidationResult = struct {
    status VerificationStatus;
    granted_tier uint8;
    root_anchor_id string:64;
};

@discoverable
protocol AppTrustManager {
    /// Validate an app binary or package manifest signature
    ValidateAppSigner(struct {
        package_id string:128;
        signer_cert_chain vector<vector<uint8>:2048>:4;
        payload_digest array<uint8, 32>; // BLAKE3 hash of .bex
        signature vector<uint8>:512;
    }) -> (struct {
        status bexos.kernel.Status;
        result AppValidationResult;
    });

    /// Ingest an Enterprise MDM signing key (Requires TIER0/MDM Authority)
    InstallEnterpriseRoot(struct {
        root_cert vector<uint8>:2048;
        permitted_prefixes vector<string:128>:8;
    }) -> (struct {
        status bexos.kernel.Status;
    });

    /// Check/Apply certificate revocation list (CRL / TUF targets)
    UpdateRevocationList(resource struct {
        revocation_payload handle:VMO;
    }) -> (struct {
        status bexos.kernel.Status;
    });
};

@discoverable
protocol TlsTrustManager {
    GetTlsRootBundle() -> (resource struct {
        status bexos.kernel.Status;
        bundle handle:VMO;
        length uint64;
        generation uint64;
        format uint8; // REDB_TLS_ROOTS
    });
};

```

## Key Invariants

* **No Web PKI Cross-Contamination:** A compromised commercial Web SSL CA (e.g., used for HTTPS websites) cannot issue valid code-signing certificates for BexOS applications.
* **Prefix Clamping:** An enterprise or store root anchor can be restricted to specific namespaces (e.g., `permitted_package_prefixes: ["com.acme.*"]`), preventing an enterprise IT key from impersonating first-party system apps or other third-party services.
* **Revocation Sync:** Revoked developer keys and compromised intermediates are synced through TUF metadata updates and persisted in `trustd`'s hardware-backed monotonic rollback store.
