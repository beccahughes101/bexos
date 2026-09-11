# RFC 0006: User identity, encrypted homes, and keychains

- Created: 2026-08-26T17:31:23-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

User authentication, encrypted home storage, and keychains share a Trusty-backed key hierarchy. Separate services own identity, secret storage, and process management; future unlock mechanisms retain the same U-KEK contract.

## Design overview

A hierarchical key-derivation and keystore topology rooted in **Trusty KeyMint and Gatekeeper** connects users, keychains, encrypted homes, and cloud SSO. The service boundaries avoid circular dependencies and preserve security isolation.

> Current implementation note: the bootfs `bexos.service.usersd` singleton
> enrolls and verifies the v1 password factor with Trusty Gatekeeper, persists
> the opaque password handle and an auth-bound KeyMint HMAC blob, and recreates
> the BexFS U-KEK only after successful authentication. `keychaind` is a
> separate service and stores opaque KeyMint blobs and encrypted envelopes.
> Cloud SSO, biometrics, passkeys, and per-user keychain processes remain
> future designs below; they are not current security claims.

In this model, user authentication unlocks a master **User Key Encryption Key (U-KEK)**. That key simultaneously unlocks the **Per-User Encrypted Storage Volume** (`/data`) and unseals the **User Keychain**.

## The Cryptographic Hierarchy

```
                      +-----------------------------+
                      |   Hardware Root of Trust    |
                      | (Trusty / TPM / Secure EL1) |
                      +-----------------------------+
                                     |
                +--------------------+--------------------+
                |                                         |
                v                                         v
+-------------------------------+         +-------------------------------+
|       System Keystore         |         |     Auth Factor Evaluator     |
| (Disk keys, Wi-Fi, TLS certs) |         | (Password, PIN, Passkey, SSO) |
+-------------------------------+         +-------------------------------+
                                                          |
                                                          | Derives / Unseals
                                                          v
                                          +-------------------------------+
                                          |   User Master KEK (U-KEK)     |
                                          |     (Held only in RAM)        |
                                          +-------------------------------+
                                                          |
                                     +--------------------+--------------------+
                                     |                                         |
                                     v                                         v
                      +-----------------------------+           +-----------------------------+
                      |    User Encrypted Home      |           |        User Keychain        |
                      |  (`fs/user_{uid}.img` key)  |           | (App tokens, Passwords, PRT)|
                      +-----------------------------+           +-----------------------------+

```

## The Two Keystores (System vs. User)

### System Keystore

* **Availability:** Unlocked at initial device boot (before user login).
* **Backed by:** Hardware Root of Trust (Trusty secure world in the ARM64 profile; a future provider may use a TPM).
* **Contents:** Base full-disk metadata keys, network credentials (Wi-Fi, 802.1X), device enrollment certificates, and IdP discovery metadata.

### User Keystore (Keychain)

* **Availability:** Sealed on disk until the user enters their PIN/password or completes SSO auth.
* **Backed by:** `redb` embedded database encrypted with `AES-256-GCM` using `U-KEK`.
* **Contents:** Application API tokens, OAuth refresh tokens, saved passwords, private SSH keys, and cloud IdP Primary Refresh Tokens (PRTs).

## Multi-Factor Unlock Pathways (Deriving `U-KEK`)

BexOS v1 implements the local password path. The other rows describe future
unlock mechanisms that must converge on the same U-KEK contract without
weakening KeyMint authorization:

| Unlock Method | Mechanism | Offline Capable? |
| --- | --- | --- |
| **Local Password (implemented)** | Gatekeeper verification yields a hardware-authentication token; authenticated KeyMint HMAC recreates `U-KEK`. | **Yes** (100% offline). |
| **Hardware Biometric / Passkey (future)** | A Trusty-compatible secure authenticator would authorize the same KeyMint-bound derivation. | **Yes**. |
| **Platform SSO (Cloud IdP)** | WebAuthn / OpenID Connect / WS-Trust token exchange unseals local `U-KEK` via Diffie-Hellman Key Exchange (Platform SSO 2.0 style). | **Yes, with cached LKG tokens**. |

## How Platform SSO & Network Login Works

Borrowing from Apple's Platform SSO 2.0:

### Initial Setup / Device Registration

* The user logs into their Google/Work account over the captive network browser.
* KeyMint generates a hardware-bound device signing key.
* The IdP returns a cryptographic assertion containing a **Wrapped User Secret**. A future Trusty factor broker binds this secret to the local account.

### Everyday Login Window

* *Online Flow:* The login window renders an isolated Web SSO view, validates MFA with Google/Apple/Entra, and receives the session ticket. A future Trusty factor path authorizes KeyMint to recreate `U-KEK`.
* *Offline Flow:* If no Wi-Fi is available, the user falls back to their local PIN/password or hardware passkey. The local PIN unlocks `U-KEK` and simultaneously caches an offline SSO PRT in the User Keychain.

### Automatic App SSO

* Once `U-KEK` is unlocked, `appd` can broker app-specific OAuth/OIDC access tokens directly to consumer apps (e.g., Google Maps, Chrome) from the User Keychain without prompting the user again.

## Mounting the Encrypted Home Sandbox

Instead of mounting a shared global path, the encrypted storage is attached directly to the per-process namespace:

* The user's home directory lives in a sparse image or sub-volume (`/var/storage/user_<uid>.img`).
* When `U-KEK` is unsealed at login, the VFS filesystem service initializes an in-memory decryption context.
* When `appd` launches an app for this user, it mounts the decrypted tree at the virtual `/data` path inside the app's capability namespace.

### Logout / Lock

* *Screen Lock:* Keychain is locked; memory pages holding encryption keys in VFS are marked non-swappable and zeroed if suspended.
* *User Logout:* All user processes are terminated, the `/data` filesystem unmounts, and `U-KEK` is completely purged from RAM.

## FIDL Authentication & Keychain Service Interface

```fidl
library bexos.identity;

protocol AccountManager {
    /// Attempt login via local factor (PIN/Password) or SSO Assertion
    AuthenticateUser(struct {
        user_id uint64,
        credential CredentialPayload
    }) -> (resource struct {
        status Status,
        session_token handle:EVENT,
        keychain_client client_end:KeychainService
    });

    /// Lock active session and purge decrypted keys from RAM
    LockSession(struct { user_id uint64 }) -> (struct { status Status });
};

protocol KeychainService {
    /// Retrieve secret item (gated by caller manifest CEL policy)
    GetSecret(struct {
        service_id string:128,
        account_name string:128
    }) -> (struct {
        status Status,
        secret_bytes vector<uint8>
    });

    /// Store a credential (e.g. App OAuth Token)
    SetSecret(struct {
        service_id string:128,
        account_name string:128,
        secret_bytes vector<uint8>,
        access_policy string:256  // CEL Expression
    }) -> (struct { status Status });
};

```

This guarantees that apps cannot access other apps' credentials, full-disk security remains decoupled from individual apps, and offline password/PIN unlock works alongside cloud-managed Platform SSO.

The `user service` owns identities and sessions, and the `keychain service` owns secrets. Both remain separate from `appd`.

Keeping them distinct enforces least privilege, simplifies multi-user management, and prevents cryptographic keys from being exposed to process management bugs.

## Why Separate Services?

### Principle of Least Privilege & Memory Isolation

* `appd` is a high-traffic broker: it parses manifests, evaluates CEL policies, resolves FIDL channels, and loads ELF/WASM code.
* `keychain service` handles high-value secrets (decrypted master keys, OAuth tokens, private keys).
* If `keychain service` runs in its own memory container, a memory vulnerability or parser bug in `appd` cannot compromise raw credentials in RAM.

### Clean Lifecycle & Session Scoping

* `appd` is a **System Singleton** (boots at Tier 0 before any user logs in to run drivers, network, and storage).
* `user service` manages the login screen, multi-factor evaluation, Platform SSO flows, and user profiles.
* `keychain service` instances can be **User-Scoped Singletons**: spawned only when a session unlocks and completely destroyed/purged from memory upon logout.

### Heart Transplant State Hygiene

* During a kernel swap, serializing `appd` (process tree + capability routing graph) is separated from cryptographic state.
* `keychain service` keeps its memory pages pinned and locked (`mlock`), avoiding accidental leakage into serialization buffers.

## Service Topology & Responsibilities

| Service | Lifecycle | Core Responsibilities |
| --- | --- | --- |
| **`appd`** | System Singleton | Package installer, manifest DB (`redb`), runner executor, CEL capability broker. |
| **`user service`** | System Singleton | Login screen coordination, user profiles (`/home/<uid>`), Platform SSO negotiation, session lock/unlock events. |
| **`keychain service`** | User-Scoped Singleton | Secure storage of tokens/passwords in memory/disk; enforces per-app CEL read/write rules. |

## How They Coordinate During Login & Execution

```
                       1. Auth Success (PIN/SSO)
   +--------------+ -----------------------------> +-------------------+
   | user service |                                | Trusty services   |
   +--------------+ <----------------------------- +-------------------+
          |                  2. Unseals U-KEK
          |
          | 3. Spawn User-Scoped Services
          v
   +--------------------+          +--------------------+
   |  keychain service  |          |    VFS Service     |
   | (holds credentials)|          | (mounts /data/<uid>|
   +--------------------+          +--------------------+
          ^                                  ^
          | (Brokers channel)                | (Mounts into namespace)
          +------------------+---------------+
                             |
                   +-------------------+
                   |    appd    |
                   +-------------------+
                             |
                             | 4. Launches App with
                             |    /data and Keychain handles
                             v
                   +-------------------+
                   |   User App Proc   |
                   +-------------------+

```

1. **Authentication:** `usersd` handles the login flow and, for implemented
   password authentication, uses Gatekeeper plus an auth-bound KeyMint HMAC to
   derive the `U-KEK`.
2. **Session Provisioning:** `user service` signals the VFS to mount the encrypted `/data` directory and spawns the user's dedicated `keychain service`.
3. **App Launch:** When launching an app, `appd` requests a scoped client endpoint from `keychain service` and injects it into the app's initial `/svc` namespace.
4. **Credential Access:** When the app requests an OAuth token via `bexos.identity.Keychain`, the request goes directly to `keychain service`, which checks the app's manifest identity and returns the secret.

A single shared partition can isolate users through **directory-level file-based encryption (fscrypt)** or **per-user sparse loopback disk images (ChromeOS-style cryptohome)**.

Both patterns avoid the need to repartition the physical disk every time a new user registers.

## Per-user sparse disk images (ChromeOS / Android style)

This mirrors how ChromeOS (`cryptohome`) and Android handle multi-user separation inside a single underlying partition.

```
STORAGE Partition (Mounted at /data via RoseFS)
├── system/                         <-- Unlocked via a provisioned machine root
│   ├── system_config.db
│   └── network_profiles/
└── vault/                          <-- Per-User Encrypted Images
    ├── user_1000.roseimg           <-- Alice's Sparse RoseFS image (AES-XTS-256)
    ├── user_1001.roseimg           <-- Bob's Sparse RoseFS image   (AES-XTS-256)
    └── guest.roseimg               <-- Ephemeral / In-Memory

```

### How It Works:

1. **Host Storage (`/data/vault`):** Each user's entire home volume is stored as a single sparse file on the shared `STORAGE` partition (e.g., `/data/vault/user_<uid>.roseimg`).

#### Key Derivation via Secure World (Trusty Gatekeeper / KeyMint)

* When Alice logs in, `usersd` verifies her password with Gatekeeper and receives a hardware-authentication token.
* `usersd` uses that token with an auth-bound opaque KeyMint HMAC key to recreate the 32-byte $U\text{-}KEK$; Trusty persists its secure state through RPMB.

#### Loopback Block & Mount

* `appd` passes the file descriptor of `user_1000.roseimg` and the $U\text{-}KEK$ to a sandboxed D1 loopback encryption service (`d1_dm_crypt` / `d1_rose_loop`).
* The decrypted virtual block device is mounted at `/home/alice` (or `/data/user/1000`).

#### Clean Unmount on Logout

* When Alice logs out, the filesystem unmounts, the loop device is destroyed, and the $U\text{-}KEK$ is wiped from RAM. Bob has zero physical capability to read Alice's underlying bytes.

## How Multi-User Coordinates with Capabilities and VFS Namespaces

With either sparse images (Option A) or directory encryption (Option B), BexOS ensures user isolation at runtime through **per-process private namespaces**:

```
[ Alice's Browser Process ]                  [ Bob's Terminal Process ]
          │                                              │
          ▼ Resolves /data/local                         ▼ Resolves /data/local
┌────────────────────────────────┐             ┌────────────────────────────────┐
│ Alice's Private Namespace VFS  │             │ Bob's Private Namespace VFS    │
│  • /data -> /home/alice/data   │             │  • /data -> /home/bob/data     │
│  • /pkg  -> /pkg (ReadOnly)    │             │  • /pkg  -> /pkg (ReadOnly)    │
└────────────────────────────────┘             └────────────────────────────────┘

```

1. **No Ambient Cross-User Access:** Process capability manifests declare that the application needs `{ storage: "user_data", path: "/data/local" }`.
2. **Namespace Binding:** When `appd` launches an app on behalf of Alice (Session UID 1000), it binds the decrypted user directory `/data/vault/user_1000_mount` directly to the app's root namespace `/data/local`.
3. **Isolation Guarantee:** An app running under Alice cannot construct a path to Bob's files because Bob's mount point does not exist within Alice's VFS capability namespace.

**Apps should be stored globally at the system level, while users perform a "soft install" (activation and permission grant) to access them.**

This architecture is used by ChromeOS and modern Android. It aligns directly with BexOS's immutable package architecture, content-addressed storage, and multi-user encryption model.

## How the Model Works

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ 1. SYSTEM-LEVEL PACKAGE REPOSITORY (/system/packages/ on RoseFS)            │
│    Holds immutable, deduplicated `.bex` archives.                           │
│    • `com.bexos.browser.bex`  (BLAKE3 Merkle Tree, Signed by Vendor)        │
│    • `com.monzo.app.bex`      (Signed by Monzo)                                      │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ Mounted read-only (/pkg)
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ 2. PER-USER SOFT ACTIVATION (Stored in User Vault / redb)                            │
│                                                                             │
│  [ Alice (UID 1000) ]                     [ Bob (UID 1001) ]                │
│  • Installed: Browser, Monzo                      • Installed: Browser only         │
│  • Grants: CAMERA = Allow                         • Grants: LOCATION = Deny                │
│  • State: /vault/user_1000/monzo/         • State: /vault/user_1001/...     │
└─────────────────────────────────────────────────────────────────────────────┘

```

### Step 1: System-Level Storage (Physical Stage)

When any user requests an application (or the OS comes pre-bundled with it):

* The `.bex` container is downloaded once to the global package pool (`/system/packages/`).
* The system verifies its Ed25519 signature and BLAKE3 Merkle tree.

* The binary is stored **once**, shared across the entire device, and kept read-only.

### Step 2: Per-User "Soft Install" (Logical Activation)

When Alice clicks "Install" on an app that is already in the system package repository:

* **No binary download or disk duplication occurs.**
* `appd` records an activation entry in Alice's user profile:

```json5
{
  "user_id": 1000,
  "package_name": "com.monzo:app",
  "enabled": true,
  "user_granted_permissions": ["LOCATION", "STORAGE_ACCESS"],
  "pinned_to_launcher": true
}

```


* `vfsd` provisions an empty, encrypted private directory for Alice: `/vault/user_1000/apps/com.monzo_app/data`.

## Benefits of per-user soft installation

| Metric | Per-User Hard Installs (Duplicate binaries per user) | System-Level Storage + Per-User Soft Install |
| --- | --- | --- |
| **Disk Space** | If 3 users install a 2GB game, it consumes **6GB** of flash storage. | Consumes **2GB** total; shared across all users. |
| **Install Speed** | User 2 must wait for the full download and disk write. | **Instant install (< 10ms)** for User 2 because the package is already cached. |
| **Security Auditing** | `appd` must scan separate directory trees across encrypted user profiles. | `updated` verifies and updates a single immutable catalog. |
| **Permissions & Privacy** | Coupled to filesystem placement. | Fully isolated: User 1 and User 2 maintain distinct permissions, tokens, and data directories. |

## What Happens on App Removal / Updates

### User Uninstalls App

* Only the soft link, user permissions, and Alice's local `/data` directory are deleted.

* If Bob still uses the app, the `.bex` file stays in `/system/packages/`.
* If no users on the system hold an active soft-install, `appd` / `vfsd` flags the `.bex` package as an unreferenced orphan and garbage-collects it from the base `RoseFS` partition during storage compaction.

### App Update

* `updated` stages the new version in `/system/packages/` and atomically updates the pointer.
* All users who have soft-installed the app automatically run the updated version on their next launch without downloading individual per-user copies.
