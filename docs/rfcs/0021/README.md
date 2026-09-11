# RFC 0021: Storage and VFS broker

- Created: 2026-08-28T12:15:27-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

Vfsd owns block-device mounts and directory capabilities, while appd decides access at launch. The design covers bootstrap, package storage, private namespaces, and removable-media access.

## Design overview

A dedicated **Storage / VFS Broker Service (`vfsd`)** managing block devices, filesystem mounts, and multi-layered namespaces offloads filesystem bookkeeping from `appd`, keeping the process lifecycle manager small and crash-resilient.

However, having the application talk to a central `vfsd` on every file operation and letting `vfsd` dynamically look up the caller's identity in `redb` breaks the object-capability model and introduces IPC bottlenecks.

The design therefore separates access decisions from mount and node ownership.

## Separation of responsibilities

Instead of querying identity on every I/O call, `appd` acts as the **capability coordinator (access decision point)** at launch, while `vfsd` acts as the **Mount & Node Provider (Resource Owner)**.

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ appd (Lifecycle & Process Supervisor)                                       │
│ 1. Spawns new process container                                             │
│ 2. Asks vfsd for capability handles:                                        │
│    • GetPackageRoot(app_id)  ──► returns handle:Directory (/pkg)            │
│    • GetUserRoot(uid, app_id) ──► returns handle:Directory (/data)           │
│    • GetSharedVault(vault_id) ──► returns handle:Directory (/shared/vault)  │
│ 3. Injects handles directly into the process's initial startup namespace    │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ Mints & routes handles
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ vfsd (Layered VFS & Storage Manager Daemon)                                 │
│                                                                             │
│ [ Namespace Layering Engine ]                                               │
│   ├── Base Layer: Read-only Root (/pkg via ArchiveFS, /deps via .bex)       │
│   ├── System Layer: Unencrypted /system configurations                      │
│   └── User Layer: Per-User Sparse RoseFS Loopback (Decrypted with U-KEK)    │
│                                                                             │
│ [ Backend Drivers ]                                                         │
│   ├── ArchiveFS (Merkle-verified .bex containers)                           │
│   ├── RoseFS Engine (Host storage & per-user loopback images)               │
│   └── DiskImage Driver (Block-level crypto & sector virtualization)         │
└─────────────────────────────────────────────────────────────────────────────┘
                                       ▲
                                       │ Zero lookup overhead
                                       │ Direct peer-to-peer FIDL channel
┌──────────────────────────────────────┴──────────────────────────────────────┐
│ App Process (e.g., com.bexos.browser)                                       │
│ • Holds `/pkg` and `/data` Directory capability handles                     │
│ • Reads/writes directly without ambient path lookups or identity overhead   │
└─────────────────────────────────────────────────────────────────────────────┘

```

## Direct capabilities and caller identity lookups

Having `vfsd` check `redb` on every `open()` or `read()` call introduces three issues:

* **Ambience & Confused Deputy:** The server must rely on caller tokens or PID sniffing, which reintroduces ambient authority.
* **Performance Penalty:** Checking a database (`redb`) on every directory traversal adds synchronization locks and latency to file reads.
* **Channel Saturation:** A single global FIDL endpoint would bottleneck under high multi-app I/O.

**The Fix:**

1. When an app launches, `appd` verifies permissions and calls `vfsd` to mint dedicated channel handles for the app's granted directories.

2. `appd` places those directory channels into the app's local namespace table at `/pkg`, `/data`, `/deps`, and `/shared`.

3. The app issues standard `Directory.Open()` and `File.Read()` requests directly on those handles. `vfsd` fulfills requests immediately without identity re-verification, because **holding the handle proves authorization**.

**Current implementation milestone:** appd initializes `vfsd` with the
STORAGE package store, launches apps with a mounted ArchiveFS `/pkg`, and asks
`vfsd` for a package-scoped writable `/data` directory rooted at
`data/users/<uid>/apps/<package_id>/data`. The deployed service is a std-linked
Tokio daemon with an async direct-channel manager loop; package-store mounts and
directory capability handles remain explicit BexOS resources preserved across
heart transplant. Wider document access, shared vaults, removable media, and
picker-minted file handles remain future capability grants. The current manager
surface also mints system-service data directories under
`data/system/<package_id>` and user home directory handles for trusted services
such as `keychain`.

## The Layered VFS Structure in `vfsd`

`vfsd` composes the storage hierarchy from the following layers:

### Layer 0 (Base System & Package Store)

* `vfsd` attaches `ArchiveFS` providers to the package archives on disk.
* Serves immutable `/pkg` code and `/deps/<package>` dependencies directly from BLAKE3-verified archives.

### Layer 1 (System Configuration)

* Serves global, unencrypted machine states and root certificates stored on the base `RoseFS` partition.

### Layer 2 (User Encrypted Overlay / Union)

* When a user logs in, `vfsd` attaches the user's `user_<uid>.roseimg` via the `DiskImage` driver and mounts it with `RoseFS`.
* `vfsd` isolates sub-trees (`/vault/user_1000/apps/<app_id>/data`) and exposes them as the root `/data` handle for that app session.

## Core FIDL Interfaces for `vfsd`

### A. Management Interface (`bexos.vfs.manager`) — Restricted to `appd`

```fidl
library bexos.vfs.manager;

using bexos.kernel;
using bexos.vfs;

@discoverable
protocol VfsManager {
    /// Mount an encrypted per-user storage image upon login
    MountUserVault(resource struct {
        uid uint32,
        image_file handle:CHANNEL,
        key_handle handle:CRYPTO_KEY
    }) -> (struct { status bexos.kernel.Status });

    /// Unmount user storage on logout and wipe keys
    UnmountUserVault(struct { uid uint32 }) -> (struct {
        status bexos.kernel.Status
    });

    /// Mint a capability handle to an app's read-only package directory
    GetPackageDirectory(struct {
        package_id string:128
    }) -> (resource struct {
        status bexos.kernel.Status,
        dir handle:CHANNEL // Implements bexos.vfs.Directory
    });

    /// Mint a capability handle to an app's isolated mutable data directory
    GetUserDataDirectory(struct {
        uid uint32,
        package_id string:128
    }) -> (resource struct {
        status bexos.kernel.Status,
        dir handle:CHANNEL // Implements bexos.vfs.Directory
    });
};

```

### B. Standard Node & File Operations (`bexos.vfs`) — Used by Apps

```fidl
library bexos.vfs;

using bexos.kernel;

protocol Node {
    GetAttr() -> (struct { status bexos.kernel.Status, attributes FileAttributes });
    Close();
};

protocol File {
    compose Node;
    Read(struct { count uint64 }) -> (struct { status bexos.kernel.Status, data vector<uint8>:8192 });
    Write(struct { data vector<uint8>:8192 }) -> (struct { status bexos.kernel.Status, actual uint64 });
    GetBuffer() -> (resource struct { status bexos.kernel.Status, vmo handle:VMO }); // Zero-copy mmap
};

protocol Directory {
    compose Node;
    Open(resource struct {
        path string:256,
        flags OpenFlags,
        object server_end:Node
    });
    ReadEntries() -> (struct { status bexos.kernel.Status, entries vector<DirEntry>:64 });
};

```

## Key Advantages of This Split

1. **`appd` Remains Lean:** `appd` only tracks process lifecycles and routes channel handles; it does not parse filesystem disk layouts or manage storage caches.

2. **Crash Resilience:** If `vfsd` encounters a filesystem assertion, `appd` can restart `vfsd` and rehydrate the open channel table without tearing down unrelated compute tasks.
3. **Zero Ambience:** An app cannot open a file outside `/data` or `/pkg` because those root directory handles are the only entry points it has into the filesystem graph.

The bootstrap sequence gives `appd` disk access without circular dependencies. External storage, including SD cards and USB drives, is mounted dynamically and exposed through scoped capabilities.

## Part 1: How `appd` Reads and Writes From Disk (The Bootstrap Sequence)

`appd` must obtain filesystem capabilities while it bootstraps the services that provide persistent storage.

`appd` starts from an in-memory **`bootfs`** (ramdisk) provided by the bootloader/kernel. It then bootstraps persistent storage in discrete phases:

```
[ Boot Phase 0: Kernel + bootfs ]
  • Kernel mounts in-memory `bootfs` (Wave 0 binaries: appd, D1 drivers, vfsd).
  • appd launches out of RAM.

[ Boot Phase 1: Driver & Storage Bring-up ]
  1. appd spawns PCIe & NVMe/eMMC D1 drivers from bootfs.
  2. NVMe driver discovers disk partitions and registers `BlockDevice` endpoints with appd.
  3. appd spawns `vfsd` (from bootfs) and hands it the physical NVMe `BlockDevice` handle.

[ Boot Phase 2: Host Filesystem Mounting ]
  4. `vfsd` mounts the physical `STORAGE` partition using `RoseFS`.
  5. `vfsd` calls back to `appd` (or responds to appd's initial setup call), handing appd
     a set of direct Directory handles:
     • Handle to `/system/packages` (where `.bex` archives live)
     • Handle to `/system/state`    (where system settings / databases live)
     • Handle to `/vault`           (where per-user encrypted images live)

[ Boot Phase 3: Steady-State Execution ]
  6. `appd` now reads package manifests and writes its database using its private `/system/state` handle.
  7. `appd` can now launch non-bootfs apps (Browser, Compositor, Shell) from disk.

```

`appd` does **not** make ambient `open("/system/...")` calls. It holds private `bexos.vfs.Directory` channel capabilities that `vfsd` minted for it during Phase 2.

For launched apps, the current appd milestone sends startup namespace
handles in path order as `/pkg;/data`: `/pkg` is read-only package content
mounted from the signed archive, and `/data` is the package-scoped mutable
directory minted by `vfsd`.

## Part 2: Handling External & Removable Storage (SD Cards, USB Drives)

Removable storage cannot be baked into static namespaces because devices are hotplugged and unplugged dynamically. The design uses **dynamic block-device discovery** and a dedicated **Storage Picker / Intent capability model**.

```
[ USB Drive / SD Card Inserted ]
               │
               ▼
[ D1 Driver (e.g. xhci_usb / sdhci_sdcard) ]
   • Detects hardware interrupt.
   • Registers new BlockDevice with `appd` / `DeviceRegistry`.
               │
               ▼
[ appd / Storage Coordinator ]
   • Tells `vfsd` to probe filesystem (FAT32, exFAT, RoseFS, Ext4).
               │
               ▼
[ vfsd (Mount & Media Manager) ]
   • Mounts filesystem at dynamic media volume (e.g., `vol_usb_8f21a4`).
   • Notifies Shell / File Manager: "New Volume: Samsung USB (32GB)".

```

## Part 3: How Apps Access the External Storage

In a strict capability system, an app **cannot** just read `/media/sdcard/photo.jpg`. There are two standard access patterns depending on the app's use case:

### Pattern A: User-Driven Access via File Picker (Default for standard apps)

For standard apps (e.g., photo editor, document viewer), access is mediated by user consent:

1. The app invokes the system **File Picker FIDL service** (`bexos.ui.FilePicker.OpenFiles()`).
2. The system File Picker UI (part of the privileged shell) displays the SD Card / USB drive.
3. The user selects `vacation.png` or an entire folder `DCIM/`.
4. The File Picker asks `vfsd` to mint a capability handle (`handle:CHANNEL` implementing `bexos.vfs.File` or `bexos.vfs.Directory`) scoped **only** to the selected item.
5. The handle is passed over IPC to the calling app. The app reads the file via the capability handle without ever knowing the SD card's physical path.

### Pattern B: Media Indexer / Direct Volume Capability (For file managers, music players)

If an app has an explicit manifest permission for external media (e.g., `@permission("bexos.permission.READ_EXTERNAL_STORAGE")`):

1. The app requests volume binding from `appd` / `vfsd`.
2. `vfsd` evaluates the app's permission and user consent.

3. `vfsd` mounts a read-only (or read/write) directory capability at a dynamic namespace path:
```
/volumes/sdcard_0/
/volumes/usb_drive_1/

```


4. If the user unplugs the SD card:
* The D1 driver drops the hardware channel.
* `vfsd` tears down the volume and closes the directory channel.
* Any active app reads return `ERR_PEER_CLOSED` or `ERR_DEVICE_REMOVED`, preventing kernel panics or data corruption.

## Summary of Component Roles

| Component | Responsibility |
| --- | --- |
| **D1 Bus Drivers** (`sdhci`, `xhci`) | Talks to physical hardware registers, handles hotplug IRQs, exports raw `BlockDevice` FIDL channels. |
| **`vfsd`** | Consumes `BlockDevice` channels, detects filesystem format (FAT32/exFAT/RoseFS), mounts volumes, and exposes `Directory`/`File` capability nodes. |
| **`appd`** | Orchestrates bootstrap order (Phases 0–3), routes storage capability handles during process spawning, and gates volume permissions. |
| **App Process** | Interacts strictly through the injected `/pkg`, `/data`, `/deps`, or dynamically picked `/volumes/...` channel handles. |

Application files are not stored in `bootfs`.

`bootfs` contains only a minimal set of binaries needed to mount the disk (around ~15–30 MB total). All actual applications, games, browser assets, and databases live on the physical storage disk (`RoseFS`).

## Clarifying What Goes Where

```
┌─────────────────────────────────────────────────────────────┐
│ IN RAM AT BOOT: `bootfs.img` (Minimal ~20MB initramfs)       │
├─────────────────────────────────────────────────────────────┤
│ ONLY contains:                                              │
│ • The microkernel ELF                                       │
│ • `appd` (Process supervisor binary)                        │
│ • `vfsd` (Storage daemon binary)                            │
│ • Wave 0 D1 drivers: PCIe Root, NVMe driver, RoseFS driver  │
└─────────────────────────────────────────────────────────────┘
                               │
                               │ Brings up NVMe and mounts physical disk
                               ▼
┌─────────────────────────────────────────────────────────────┐
│ ON PHYSICAL DISK: `STORAGE` Partition (RoseFS)              │
├─────────────────────────────────────────────────────────────┤
│ Holds EVERYTHING else:                                      │
│ • `/system/packages/*.bex` (Browser, Spotify, VS Code, etc) │
│ • `/system/state/`         (appd's persistent redb state)   │
│ • `/vault/user_*.roseimg`  (All user mutable files & data)  │
└─────────────────────────────────────────────────────────────┘

```

## Where and How `appd` Stores What It Needs

`appd` needs persistent storage for:

1. Installed package metadata (package names, trust roots, signing certs, CEL permissions).

2. Driver bind rules catalog.

3. Last-Known-Good (LKG) update state for Heart Transplants.

### Step-by-Step Storage Bootstrap for `appd`:

#### Before the Disk is Mounted (In-Memory Phase)

* When the machine boots, `appd` runs entirely out of memory (`bootfs`).

* It holds zero persistent state and does not yet know about non-system apps.
* Its only job at this second is: **spawn the NVMe driver and `vfsd`**.

#### The Disk Mount Handshake

* The NVMe driver gives `vfsd` a handle to the physical disk.
* `vfsd` mounts the physical `STORAGE` partition using `RoseFS`.
* `vfsd` immediately calls `appd` over a startup FIDL channel and hands `appd` a dedicated directory handle:
```
appd_system_state_handle ──► Points directly to `/system/state/` on the physical RoseFS disk

```


#### `appd` Opens Its Database on Disk

* `appd` passes that directory handle to its internal storage engine (`redb` / embedded key-value store).

* `appd` loads its state database (`/system/state/app_registry.db`).

* Now `appd` has access to all persistent data, installed package lists, and manifests.

#### Launching Real Applications from Disk

To launch the browser (`com.bexos.browser`):

1. `appd` asks `vfsd` to open `com.bexos.browser.bex` from `/system/packages/` on the physical disk.

2. `vfsd` mounts the `.bex` package as `ArchiveFS`.

3. `appd` spawns the browser WASM runner and passes the `/pkg` directory handle.

## Design summary

* **`bootfs`:** Contains **zero** user apps. It is discarded or kept read-only after the physical NVMe driver and `vfsd` boot.

* **`appd`'s Storage:** `appd` stores all its metadata, permissions, and `redb` state inside `/system/state/` on the physical disk via a directory capability handed to it by `vfsd` during boot.
