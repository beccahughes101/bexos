# RFC 0009: Storage bootstrap and partition layout

- Created: 2026-08-26T20:50:50-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

A two-phase bootstrap moves from a minimal in-memory BootFS to persistent storage. The design covers partition layout, encrypted user storage, package archives, and coordination with heart-transplant updates.

## Design overview

QEMU AArch64 boots in two phases. A small read-only BootFS is placed at a fixed
RAM address by QEMU's generic loader. A versioned address-and-length descriptor
at the handoff address identifies it; the kernel rejects an absent, oversized,
malformed, or overlapping image. There is no embedded BootFS fallback.

The kernel creates the first EL0 process from
`/boot/pkg/bexos.platform.appd/bin/appd`. It gives that process a
read-only BootFS VMO and only the bootstrap capabilities required to create
services and delegate hardware.

```text
external BootFS -> kernel -> appd
                                |
              PCI + PL011 -> NVMe -> BexFS
                                |
                  SYS_STATE, /pkg, /data mounted
                                |
         copy retained inputs, close /boot, reclaim BootFS pages
                                |
                    launch disk-only verification ELF
```

`appd` advances a wave only after the launched services report
readiness. PCI enumerates the QEMU controller and hands the controller
capability to NVMe. NVMe uses mapped MMIO and pinned DMA buffers, with bounded
completion polling for this QEMU profile. BexFS consumes the block protocol
across process boundaries, splits oversized requests, and acknowledges `Sync`
only after NVMe flush completes. A readiness, mount, or I/O failure leaves
BootFS retained and prevents the pivot success marker.

## QEMU disk layout

The generated GPT preserves this order: `ESP`, `BOOT_A`, `BOOT_B`,
`SYS_STATE`, and `STORAGE`. The compact QEMU image uses 32 MiB for encrypted
`SYS_STATE` and 256 MiB for encrypted `STORAGE`. The two BexFS namespace
snapshots each have just under 128 MiB of capacity.

| Partition | QEMU use | Namespace / access |
| --- | --- | --- |
| `SYS_STATE` | Private boot state, including `SysStateV1` | system services only |
| `STORAGE` | BexFS volume containing `pkg/` package files and `data/` persistence | `/pkg` read-only, `/data` writable |

The QEMU image uses an explicitly test-only key contract. Its unified storage
volume is BexFS; package archive verification is a later production feature.
The verifier runs only from disk after BootFS reclamation; it proves that the
storage driver stack mounted successfully, `/pkg` cannot be modified, guest
writes survive reboot, and reclaimed physical pages are reusable.

## What this does not claim

QEMU directly loads the kernel, BootFS, and handoff descriptor through the
generic loader. It does not implement a production boot manager, signed boot
bundles, A/B selection, encrypted production keys, Merkle BlobFS, interrupt-
driven storage I/O, or live-update persistence. Polling, fixed addresses, CPU0
EL0 runtime launch, and the test key are bring-up constraints, not the
production partition design.

## Phase 2: The Filesystem Pivot

Once the NVMe and filesystem drivers signal readiness over FIDL:

### Namespace Overlay / Repointing

`appd` dynamically attaches the real on-disk filesystems:
* `/pkg` is bound to the package subtree on encrypted `STORAGE` with read-only access.
* `/data` is bound to the persistent data subtree on encrypted `STORAGE` with writable access.

### Reclaiming Boot RAM

`appd` closes its handle to the initial `bootfs.img` VMO. The microkernel unmaps the physical pages and returns the early boot RAM to the system memory allocator.

## How Heart Transplant Updates the Boot Drivers

Because the early boot drivers (`nvme_driver`, `bexfs`) run as standard userspace processes under `appd`:

* **During Runtime:** Updated versions of these drivers are fetched from disk (`/pkg`) or network and updated live in memory via the **Userspace Heart Transplant** protocol without touching physical storage state.
* **For the Next Cold Boot:** When a driver package is updated on disk, an atomic maintenance task updates the fallback `bootfs.img` on the EFI System Partition so the next cold start uses the new driver binary.

A standard **GPT (GUID Partition Table)** layout with **A/B system slots** ensures fail-safe cold boots, cryptographic immutability for system binaries, and isolated encrypted storage for user data.

## Recommended GPT Disk Partition Scheme

| Partition | Label | Filesystem / Format | Recommended Size | Access / Security | Purpose |
| --- | --- | --- | --- | --- | --- |
| **1** | `ESP` | FAT32 | 256 MB – 512 MB | Read-Only (runtime) | UEFI bootloader (`BOOTX64.EFI` / `BOOTAA64.EFI`), device tree blobs, and boot manager. |
| **2** | `BOOT_A` | Raw Image (Signed) | 128 MB – 256 MB | Read-Only (Secure Boot) | Active boot bundle: Microkernel ELF, Trusty/TEE image, and initial `bootfs.img`. |
| **3** | `BOOT_B` | Raw Image (Signed) | 128 MB – 256 MB | Read-Only (Secure Boot) | Standby boot bundle for A/B cold-boot fallback and staged offline OS updates. |
| **4** | `SYS_STATE` | Encrypted `BexFS` | 32 MB – 64 MB | Machine Key (TPM / RPMB) | Non-volatile A/B slot state (tries remaining, active slot, LKG version), enrollment keys, and crash logs. |
| **5** | `STORAGE` | `BexFS` | Remainder of Disk | Base System Key + Per-User Keys | Mutable user data, application config databases, browser caches, and per-user encrypted home images (`/data`). |

## Partition Deep Dive

**1. `ESP` & `BOOT_A` / `BOOT_B` (The Resilient Boot Pipeline)**

* The UEFI firmware loads the boot manager from the `ESP`.
* The boot manager queries `SYS_STATE` (or hardware RPMB) to determine the active slot (`BOOT_A` vs. `BOOT_B`).
* Each boot partition contains a single signed, cryptographically hashed image combining:
* Microkernel executable
* `tee.bin` Secure World firmware
* `bootfs.img` (containing early PCIe, NVMe, and filesystem bootstrap drivers)

* **Cold-Boot Safety:** If a cold-booted kernel panics before reaching the first successful heartbeat, the hardware watchdog/bootloader resets the machine directly into the alternate slot.

**3. `STORAGE` (Encrypted, Transactional User Volume)**

* Formatted with **`BexFS`** (Log-structured Copy-on-Write).
* Structured as a sparse container hosting isolated per-user sub-volumes:
* `/vault/system`: Machine-wide persistent daemon state and network configurations (unlocked via Hardware Device Key).
* `/vault/user_<uid>`: Per-user home directories. In the implemented Trusty
  profile, `usersd` recreates the corresponding `U-KEK` only after Gatekeeper
  verification, using an authenticated KeyMint HMAC operation; the raw U-KEK
  is never persisted.
* Sequential append-only writes maximize NVMe SSD lifespan and eliminate filesystem corruption on sudden power loss.

**4. `SYS_STATE` (Encrypted boot state)**

* The compact QEMU profile allocates exactly 32 MiB and formats it as BexFS.
* `/boot_state.bin` is the versioned `SysStateV1` record described in
  `file-systems.md`; it is not a raw partition structure and is not redb-backed.
* The mount is restricted to the GPT extent and unlocked through the same
  read-only 256-bit key VMO contract used for other BexFS volumes.
* The boot-state update is durable only after BexFS metadata, roseFS state, the
  block request stream, and the NVMe flush complete.

## How A/B Partitioning Coordinates with Heart Transplants

* **Runtime Updates (Zero Downtime):** Live microkernel, driver, and system service updates are applied entirely in RAM via the **Heart Transplant** protocol.
* **Cold-Boot Persistence:** Once a live Heart Transplant succeeds, `appd` asynchronously writes the newly verified boot bundle to the inactive slot (e.g., `BOOT_B`) and updates the `SYS_STATE` pointer.
* If the system ever loses power or undergoes a hard restart, it boots straight into the latest verified OS version without needing an update migration step during startup.

## The Unified Storage Architecture

This approach uses a single mutable, general-purpose `RoseFS` data partition:

```
[ Disk Partition: STORAGE (RoseFS) ]
  ├── system/
  │     └── packages/                      <-- Stored on RoseFS as normal files
  │           ├── com.bexos.browser.bex     <-- Immutable signed archive container
  │           ├── com.bexos.terminal.bex
  │           └── com.google.drive.bex
  └── vault/
        └── user_1000.roseimg              <-- User private loopback volumes

```

## How App Archive Mounts Work (Container Format)

A `.bex` package is an uncompressed or zstd-compressed signed container with a
fixed little-endian v1 layout:

```
BEXARCV1 header
directory table: sorted relative paths, modes, compression, sizes, offsets
stored file payloads: none or zstd compression
4 KiB BLAKE3 chunk hashes for each stored payload
BEXSIGV1 footer: signed_len, key_id, content_root, Ed25519 signature

```

When `appd` (or `appd`) launches an application:

1. **Signature Verification:** ArchiveFS verifies the footer signature against
   baked-in trusted public keys. The QEMU profile currently bakes in only the
   checked-in test key under `//lib/app_archive`.
2. **Read-Only VFS Mount:** `appd` opens `pkg/<package>.bex` from
   read-only STORAGE and asks ArchiveFS to mount it as a directory tree.
3. **Namespace Binding:** The mounted archive root is attached to the target
   process’s private namespace at `/pkg`.

Integrity verification is retained without `BlobFS`: each archive contains its own chunk hash tree, as with **`dm-verity`** or **BLAKE3 chunk hashing**:

* **Read-Time Verification:** ArchiveFS checks each stored payload's 4 KiB
  BLAKE3 chunk hashes before returning file bytes or backing memory.
* **Tamper Prevention:** If a byte on the underlying RoseFS disk was altered (bit-rot or local tampering), the page hash check fails instantly, preventing execution of modified code.

## Layered Storage Architecture

```
[ appd / Namespace Router ]
         │
         ├── For User Home Mounts (/data/user/1000)
         │     │
         │     ▼
         │   [ RoseFS Filesystem Driver ]
         │     │ Consumes bexos.storage.block.BlockDevice
         │     ▼
         │   [ D1 DiskImage Driver (Virtual Block Loopback) ]
         │     • Decrypts AES-XTS blocks with U-KEK
         │     • Reads/writes 4KB sectors on backing user_1000.roseimg file
         │     │
         │     ▼
         │   [ Base RoseFS Driver ] (Host storage partition)
         │
         └── For App Package Mounts (/pkg)
               │
               ▼ Direct VFS Mount (No intermediate Block Device needed)
             [ ArchiveFS Provider ]
               • Verifies Merkle tree / BLAKE3 chunk hashes on demand
               • Exposes virtual directory tree directly over FIDL VFS protocols
               • Backed by read-only VMOs / mmap of package.bex

```

## The `DiskImage` Virtual Block Provider

The `DiskImage` driver is a D1 block device server. It translates block reads and writes (`bexos.storage.block.BlockDevice`) into file offsets on an underlying backing file (`user_<uid>.roseimg`).

* **Block Interface:** Exposes the exact same FIDL interface as physical NVMe or VirtIO block drivers.
* **Inline Cryptography:** Optionally binds a hardware-backed key ($U\text{-}KEK$) to perform real-time block encryption (e.g., AES-XTS or ChaCha20-Poly1305) before passing writes down to the host filesystem.
* **Zero Host Awareness:** The `RoseFS` driver running Alice’s home directory has no idea it is running inside a file—it just talks to a standard `BlockDevice` endpoint.

```fidl
library bexos.storage.diskimage;

using bexos.kernel;
using bexos.storage.block;

@discoverable
protocol DiskImageManager {
    /// Create a virtual block device backed by an open file handle
    Attach(resource struct {
        file_handle handle:CHANNEL,    // FIDL file protocol to .roseimg
        key_handle handle:CRYPTO_KEY?, // Optional U-KEK for sector encryption
        read_only bool
    }) -> (resource struct {
        status bexos.kernel.Status,
        block_device handle:CHANNEL    // Implements bexos.storage.block.BlockDevice
    });
};

```

## The `ArchiveFS` Package Provider

Unlike user data (which requires read/write sector access and transactional file metadata), app packages are **immutable, content-addressed, and read-only**. Running a full block device $\to$ filesystem stack for every `.bex` package adds unnecessary queue overhead.

`ArchiveFS` serves files directly at the BexOS filesystem protocol level:

* **Direct FS Export:** `ArchiveFS` reads the directory table inside the `.bex`
  file and responds to existing `bexos.fs.Directory` and `bexos.fs.File`
  protocols over FIDL.
* **On-Demand Payload Verification:** Reads validate the archive signature,
  content root, and per-payload BLAKE3 chunk hashes before exposing bytes.
* **Shared Memory:** `GetBackingMemory` returns immutable file bytes through a
  read-only VMO after verification.

```fidl
library bexos.storage.archive;

protocol ArchiveManager {
    MountPackage(resource struct {
        package_file handle:CHANNEL;          // bexos.fs.File for package.bex
        expected_merkle_root vector<uint8>:32; // empty or exact content root
    }) -> (resource struct {
        status int32;                 // bexos.fs.FsStatus numeric value
        root_dir handle:CHANNEL;      // implements bexos.fs.Directory
    });
};

```

The intended production layout remains an ESP plus signed `BOOT_A` and
`BOOT_B` bundles, private authenticated `SYS_STATE`, and encrypted `STORAGE`.
Package execution integrity comes from immutable signed archives with embedded
Merkle verification, not a separate package partition. Those production
controls must be added independently of this QEMU bootstrap.
