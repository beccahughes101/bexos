# RFC 0025: TUF-backed update delivery

- Created: 2026-08-29T11:48:23-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

A unified TUF-backed delivery pipeline updates applications, drivers, trusted apps, and platform images. Updated verifies and stages content, while appd and platform services coordinate activation and recovery.

## Design overview

Implementing a dedicated **`updated`** backed by **The Update Framework (TUF)** establishes a resilient update subsystem for BexOS. TUF protects the OS against key compromise, rollbacks, freeze attacks, and man-in-the-middle exploits across both consumer apps and core platform images.

## Update Engine Architecture

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ updated (Unprivileged Update & Orchestration Daemon)                  │
├─────────────────────────────────────────────────────────────────────────────┤
│ 1. TUF Client Engine: Verifies root.json, targets.json, timestamp, snapshot │
│ 2. Download Pipeline: Resumable chunk streaming over connectRPC / HTTPS     │
│ 3. Staging Engine: Writes to staging buffers, verifies BLAKE3 Merkle roots  │
│ 4. Heart Transplant Coordinator: Signals appd and Secure World Orchestrator │
└──────────────────────┬────────────────────────────────┬─────────────────────┘
                       │                                │
        [ For Apps & Userspace Services ]        [ For Kernel / TEE / Drivers ]
                       │                                │
                       ▼                                ▼
        ┌─────────────────────────────┐  ┌───────────────────────────────────┐
        │ appd                        │  │ Trusty BexOS Orchestrator TA      │
        │ • Userspace Heart Transplant│  │ • Staged kernel.img / tee.bin     │
        │ • Seamless channel rebinding│  │ • RPMB anti-rollback monotonic ctr│
        │ • Atomic .bex file replace  │  │ • Two-stage kernel swap / Fallback │
        └─────────────────────────────┘  └───────────────────────────────────┘

```

## The TUF Metadata Hierarchy

Every app manifest and the OS platform manifest point to a trusted TUF repository:

```protobuf
// Snippet from app manifest / platform configuration
update_policy {
  tuf_mirror_url: "https://distribution.bexos.org/targets"
  tuf_root_keys: [
    "ed25519:3b94a1b0d70f882b5..."
  ]
  auto_apply: HEART_TRANSPLANT_SEAMLESS
}

```

When checking for updates, `updated` resolves TUF metadata roles in strict order:

1. **`root.json`:** Validates signing keys and revocations.
2. **`timestamp.json`:** Prevents freeze attacks (guarantees freshness).
3. **`snapshot.json`:** Prevents mix-and-match attacks across inconsistent target versions.
4. **`targets.json`:** Lists candidate `.bex` files, `kernel.img`, and `tee.bin` alongside their expected size, BLAKE3 root hash, and Ed25519 signatures.

## Updating Applications (Userspace Heart Transplant)

For standard apps and userspace services (like `netstack` or `com.bexos.browser`):

1. **Background Staging:** `updated` downloads the new `.bex` package to an ephemeral staging file (`/system/packages/browser.bex.staged`) on `RoseFS`.
2. **Integrity Validation:** Verifies the package's Ed25519 signature and BLAKE3 chunk tree.
3. **Atomic Replace:** Renames the staged file over the existing package path.

### Heart Transplant Handshake

* `updated` notifies `appd`: `"Package com.bexos.browser updated to v2.1.0"`.
* If the app supports state serialization, `appd` quiesces active IPC channels, serializes the app's session state into a temporary VMO, spawns the new WASM binary, rebinds the channel endpoints, and resumes execution.

## Updating Microkernel, TEE (`tee.bin`), and Drivers

Core platform components require coordination with the **BexOS orchestrator
TA running under Trusty**. Its authority is limited to secure-core update and
lifecycle coordination; KeyMint, Gatekeeper, AVB, storage, and AuthMgr remain
independent upstream services.

```
[ updated ]                 [ Orchestrator (Secure World) ]       [ Microkernel ]
       │                                        │                             │
       ├─ 1. Verifies TUF targets.json ─────────┤                             │
       │     (kernel.img, tee.bin, d1_gpu.img)  │                             │
       │                                        │                             │
       ├─ 2. Allocates Staging VMOs ───────────>│                             │
       │     and copies new binaries            │                             │
       │                                        │                             │
       ├─ 3. UpdateStagePlatform() ────────────>│                             │
       │                                        ├─ Validates Platform Certs   │
       │                                        ├─ Increments RPMB Counter    │
       │                                        │                             │
       │                                        ├─ 4. Initiates Heart ───────>│ (Enters "SWITCH" mode)
       │                                        │     Transplant Sync                │ (Streams capability state)
       │                                        │<─ 5. Sync Complete         ────────┤
       │                                        │                             │
       │                                        ├─ 6. Swaps Execution ───────>│ (New Kernel Takes Over)
       │<─ 7. Transplant Succeeded ─────────────┤

```

* **Anti-Rollback Protection:** The Orchestrator writes the minimum version counter into secure non-volatile storage (e.g., eMMC/NVMe Replay Protected Memory Block — RPMB), preventing attackers from forcing the machine back to a vulnerable older kernel.
* **TEE Applications (Trusted Apps):** A future production update flow may
  stage an authenticated Trusty application bundle, but it must preserve the
  upstream service boundaries and the orchestrator's narrow lifecycle role.
  Dynamic TA replacement is not part of the current QEMU implementation.
* **A/B Slot Synchronization:** After a live kernel heart transplant is confirmed healthy, `updated` writes the new kernel and `tee.bin` to the offline `BOOT_B` partition to ensure the system remains bootable on cold reboots.

## `updated` FIDL Protocol

```fidl
library bexos.update;

using bexos.kernel;

type UpdateType = strict enum : uint8 {
    APP_PACKAGE     = 1;
    CORE_DRIVER     = 2;
    MICROKERNEL     = 3;
    SECURE_WORLD_TEE= 4;
};

type UpdateStatus = strict enum : uint8 {
    CHECKING        = 1;
    DOWNLOADING     = 2;
    VERIFYING_TUF   = 3;
    STAGING         = 4;
    APPLYING_TRANSPLANT = 5;
    COMPLETED       = 6;
    FAILED          = 7;
};

@discoverable
protocol UpdateEngine {
    /// Query update server for latest TUF manifests
    CheckForUpdates() -> (struct {
        status bexos.kernel.Status,
        updates_available bool
    });

    /// Stream an update and stage it for execution
    ApplyUpdate(struct {
        target_package_id string:128,
        update_type UpdateType
    }) -> (struct {
        status bexos.kernel.Status
    });

    /// Event subscription for UI progress bars and system status icons
    -> OnUpdateProgress(struct {
        target string:128,
        status UpdateStatus,
        bytes_downloaded uint64,
        total_bytes uint64
    });
};

```

## Advantages of This Design

* **TUF Security Assurances:** Guarantees that app repositories and OS update mirrors cannot serve stale, mismatched, or maliciously modified packages.
* **Centralized Orchestration:** `appd` does not need networking or TUF parsing logic; it reacts to update events and applies local lifecycle changes.

* **Unified Pipeline:** App packages, userspace drivers, Trusted Apps, and the microkernel share a single verifiable delivery mechanism.
