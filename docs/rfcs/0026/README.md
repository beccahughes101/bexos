# RFC 0026: Cryptography and keychain services

- Created: 2026-08-29T12:36:40-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

In-process libraries perform bulk cryptography; isolated keychain services manage secrets and authorization. Trusty-backed operations keep private key material outside normal-world storage.

## Design overview

General cryptography belongs in an in-process library. A dedicated service (`keystored`/`keychain`) owns key lifecycle and isolation; bulk cryptographic operations should not cross IPC boundaries.

## The Architecture: Library vs. Service Separation

Putting general cryptographic functions (like hashing, stream ciphers, or symmetric bulk encryption) behind an IPC service introduces performance bottlenecks through context switching and data copying.

The industry standard architecture (matching iOS/macOS Keychain + CryptoKit, Android Keystore + BoringSSL, and Fuchsia KMS) uses a **three-tier model**:

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ 1. In-Process Crypto Library (`bexos-crypto` / Rust crate / Wasm library)   │
├─────────────────────────────────────────────────────────────────────────────┤
│ • Runs inside the caller's address space.                                   │
│ • Handles high-throughput operations: BLAKE3, ChaCha20, AES-GCM, Ed25519.  │
│ • Zero IPC overhead; operates directly on process memory buffers.           │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ Needs hardware key / secure storage
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ 2. System Keystore Service (`keychain` / `keystored` via FIDL)              │
├─────────────────────────────────────────────────────────────────────────────┤
│ • System-wide daemon managing key metadata, ACLs, and persistence.          │
│ • Stores secrets in per-user encrypted `redb` vaults.                       │
│ • Enforces caller permissions (e.g., "Only App X can use Key Y").           │
│ • Never exposes raw private keys to userspace apps.                         │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ Hardware-backed roots
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ 3. Hardware / TEE Anchor (`teed` / Trusty / Secure Enclave)                 │
├─────────────────────────────────────────────────────────────────────────────┤
│ • Generates and holds non-exportable hardware master keys (U-KEK, Device Key)│
│ • Performs isolated signing and key derivation inside Secure World.         │
└─────────────────────────────────────────────────────────────────────────────┘

```

## Why the Distinction Matters

| Operation | Where It Belongs | Why |
| --- | --- | --- |
| **Bulk Data Encryption (RoseFS, TLS, App Data)** | **In-Process Library** | Pushing hundreds of megabytes over IPC to encrypt them would destroy I/O throughput. |
| **Password / Secret Storage** | **Keychain Service** | Needs persistent storage, ACLs, access prompting, and isolation from app uninstalls. |
| **Private Key Operations (Signing, Handshakes)** | **Keychain / TEE** | The private key never enters the untrusted app's memory; the app sends a hash over FIDL and receives a signature. |
| **Hardware Root Derivation (U-KEK)** | **TEE Service (`teed`)** | Bound to hardware fuses (RPMB / OTP) so keys cannot be extracted even with root storage access. |

## Designing the `Keychain` FIDL Protocol

The `Keychain` service handles **secrets management and capability-gated signing**, delegating bulk operations to client-side libraries:

The current implementation milestone uses two normal-world keychain vaults:

* **System keychain:** a redb vault in the general storage partition for
  machine/system credentials, currently
  `data/system/bexos.service.keychaind/keychain.redb`.
* **User keychain:** one redb vault per unlocked encrypted user home, e.g.
  `data/users/<uid>/keychain.redb`.

Records inside either vault are keyed by `alias` only. User separation comes
from the encrypted-home vault location and service routing, not from a
`(uid, alias)` database key. Hardware-backed Ed25519 key requests are now
brokered by `keychaind` through `teed` to Trusty KeyMint. Normal-world storage
contains aliases, characteristics, public material, opaque KeyMint blobs, and
KeyMint-encrypted secret envelopes, never private key material. `usersd`
obtains Gatekeeper hardware-authentication tokens and derives each BexFS U-KEK
with an auth-bound KeyMint HMAC key; U-KEKs do not pass through `keychaind`.

Current QEMU implementation note: the Trusty firmware build includes KeyMint,
Gatekeeper, storage, AVB, AuthMgr FE/BE, and the retained orchestrator. Hardware
operations fail closed if the Trusty transport or RPMB backend is unavailable;
they never fall back to software-held private keys.

The shared `//lib/redb` target is compiled without `std` using redb's
experimental no-std API gate; host-only test and tool targets use
`//lib/redb:redb_host` when they need `std`.

```fidl
library bexos.security.keychain;

using bexos.kernel;

type KeyAlgorithm = strict enum : uint8 {
    ED25519     = 1;
    ECDSA_P256  = 2;
    AES_256_GCM = 3;
};

type KeyFlags = strict bits : uint16 {
    REQUIRE_USER_AUTH   = 0x0001; // Prompt user biometric/passcode on use
    EXPORTABLE          = 0x0002; // Can caller export raw key bytes?
    HARDWARE_BACKED     = 0x0004; // Enforce generation inside TEE
};

@discoverable
protocol Keychain {
    /// Store an encrypted secret (e.g., app API token, password)
    StoreSecret(struct {
        alias string:128;
        secret vector<uint8>:4096;
        flags KeyFlags;
    }) -> (struct {
        status bexos.kernel.Status;
    });

    /// Retrieve a previously stored secret (subject to ACL / prompt)
    GetSecret(struct {
        alias string:128;
    }) -> (struct {
        status bexos.kernel.Status;
        secret vector<uint8>:4096;
    });

    /// Generate a non-exportable signing key inside the Keystore/TEE
    GenerateKey(struct {
        alias string:128;
        algorithm KeyAlgorithm;
        flags KeyFlags;
    }) -> (struct {
        status bexos.kernel.Status;
        public_key vector<uint8>:512;
    });

    /// Sign data using a key held in the Keystore/TEE (key never exposed)
    Sign(struct {
        alias string:128;
        digest vector<uint8>:64; // Pre-hashed using client-side BLAKE3/SHA256
    }) -> (struct {
        status bexos.kernel.Status;
        signature vector<uint8>:512;
    });
};

```

## Design summary

1. **Keep crypto algorithms as a pure Rust library (`bexos-crypto`)** compiled directly into apps, drivers, and services for maximum speed.
2. **Make `keychain` a dedicated FIDL service** responsible for ACLs, password/token storage, and brokering hardware signing via the TEE.
3. Apps hash their data in-process using the library, and only pass small digests or secret tokens across IPC to the `keychain` service.
