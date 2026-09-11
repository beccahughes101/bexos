# RFC 0032: Application versions, pinning, and rollback

- Created: 2026-08-30T10:41:36-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

Structured versions, immutable archives, active pins, and execution policies support multiple installed application versions. Watchdog and pruning rules preserve rollback candidates.

## Design overview

Multi-version app installations combine execution policies, state pinning, and fail-safe rollback.

Current implementation status: BexOS now has structured `bexos.version`
metadata, manifest-level `package_version` and `multi_version_policy` fields,
registry records keyed by package plus `SemVer`, first-class active pin records
in checkpoint and active SYS_STATE slot storage, versioned VFS archive paths,
partial package/version dependency matching, exact package listing through
`bexos.app.version_manager.VersionManager`, allocated-byte archive accounting,
inactive-version pruning, and protected single-active crash-loop restart and
rollback automation. The current watchdog is exit/readiness based; explicit
hang heartbeat reporting, physical bootloader slot rollback, picker UI, and
durable reboot survival for memory-only heart-transplant replacement archives
remain separate future work.

## Version Protobuf Schema (`version.proto`)

```protobuf
syntax = "proto3";

package bexos.version;

message SemVer {
  uint32 major = 1;
  uint32 minor = 2;
  uint32 patch = 3;
  uint32 build = 4;
  string prerelease = 5; // e.g., "alpha.1", "rc.2"
}

enum MultiVersionPolicy {
  SINGLE_ACTIVE_ONLY   = 0; // Default: Only one version active at a time
  PARALLEL_EXECUTION   = 1; // Side-by-side execution (isolated storage per version)
  SHARED_STORAGE_MULTI = 2; // Side-by-side execution sharing primary /data storage
}

message PackageVersionMetadata {
  string package_id = 1;         // e.g. "com.monzo:monzo"
  SemVer version = 2;
  string content_blake3 = 3;     // Content-addressed hash of the .bex payload
  MultiVersionPolicy policy = 4;
  uint64 installed_timestamp = 5;
  uint32 min_bexos_abi_version = 6;
}

```

App manifests import this schema and declare structured version metadata:

```protobuf
package_name: "com.bexos.lib.react_native"
package_version { major: 0 minor: 74 patch: 3 build: 12 }
multi_version_policy: PARALLEL_EXECUTION
min_bexos_abi_version: 1
package_kind: LIBRARY
```

## On-Disk Layout on the `STORAGE` Partition

On the `STORAGE` partition, application payloads are stored immutably by their **fully qualified package identifier and content hash/version**, enabling multiple versions to co-exist without colliding.

```
/storage/packages/
  ├── com.monzo:monzo/
  │     ├── 1.2.0-b104/
  │     │     ├── pkg.bex              # Immutable read-only ArchiveFS package
  │     │     └── manifest.pb
  │     └── 1.3.0-b201/
  │           ├── pkg.bex
  │           └── manifest.pb
  └── com.bexos.lib.crypto/
        ├── 2.0.0-b12/pkg.bex
        └── 2.1.0-b18/pkg.bex

```

The implemented VFS path for versioned package archives is
`pkg/<package_id>/<semver>/pkg.bex`; legacy unversioned archives remain readable
as `pkg/<package_id>.bex`.

## State Pinning in `SYS_STATE` (A/B Partition Bound)

To protect the system against corrupt app updates, critical driver regressions, or broken system apps, `SYS_STATE` tracks pinned active package manifests tied to the active OS boot slot (Slot A or Slot B).

```

The current SYS_STATE code defines slot-scoped pinned-app registry paths and a
versioned active-pin record codec. appd carries active pin state in the
registry and reconstructs pins during migration from package records.
┌─────────────────────────────────────────────────────────────────────────────┐
│ SYS_STATE: Active Slot App Pin Registry (/sys_state/slot_a/pinned_apps.redb) │
├─────────────────────────────────────────────────────────────────────────────┤
│ Primary Key: `package_id`                                                   │
│ Record:                                                                     │
│ • `pinned_version`: "1.2.0-b104"                                            │
│ • `content_blake3`: "9f8a6b..."                                             │
│ • `is_critical_boot_app`: true/false                                        │
│ • `health_check_status`: HEALTHY | PROBATION | CRASH_LOOP                   │
│ • `rollback_target_version`: "1.1.9-b98"                                    │
└─────────────────────────────────────────────────────────────────────────────┘

```

## Multi-Version Execution Policies

`appd` evaluates how to mount storage and invoke processes based on `MultiVersionPolicy`:

### `SINGLE_ACTIVE_ONLY` (Default for Standard Apps)

* Multiple versions remain installed on the `STORAGE` partition.
* Only one version is marked `ACTIVE` in `pinned_apps.redb`.
* Installing version `1.3.0` sets it as active, but keeps `1.2.0` on disk for immediate instantaneous rollback without re-downloading.

### `PARALLEL_EXECUTION` (Side-by-Side Isolation)

* Both `1.2.0` and `1.3.0` run concurrently.
* `vfsd` assigns isolated home vault namespaces:
* `/vault/user_1000/apps/com.monzo:monzo/v1.2.0/data`
* `/vault/user_1000/apps/com.monzo:monzo/v1.3.0/data`

### `SHARED_STORAGE_MULTI` (Developer & Canary Channels)

* Both versions run concurrently, but share `/vault/user_1000/apps/com.monzo:monzo/shared/data`.

## Automated Rollback & Watchdog Architecture (A/B & Slot Recovery)

When a critical app or driver is upgraded, `appd` places it into a **probation window**:

```
[ New Version 1.3.0 Activated on Slot A ]
                   │
                   ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ 1. Probation Window Starts (e.g., 60 seconds)                               │
│    • App/driver is launched in health probation state                       │
│    • Heartbeat expected via `AppLifecycle.ReportHealthy()`                  │
└──────────────────┬──────────────────────────────────────────────────────────┘
                   │
         ┌─────────┴─────────┐
         │                   │
   [ Heartbeat OK ]    [ Crash Loop / Panic / Hang ]
         │                   │
         ▼                   ▼
┌──────────────────┐ ┌────────────────────────────────────────────────────────┐
│ Mark Permanent   │ │ Automated Rollback Flow:                               │
│ • Clear probation│ │ 1. App-Level Rollback:                                 │
│ • Mark 1.3.0 as  │ │    Re-point `pinned_apps.redb` to 1.2.0 immediately.   │
│   HEALTHY        │ │ 2. Critical/System-Level Rollback (Fatal System App):  │
│ • Safe to GC old │ │    Trigger A/B bootloader fallback; boot Slot B with   │
│   version later  │ │    verified previous known-good state.                 │
└──────────────────┘ └────────────────────────────────────────────────────────┘

```

## `bexos.app.VersionManager` FIDL Protocol

```fidl
library bexos.app;

using bexos.kernel;
using bexos.version;

type AppVersionEntry = struct {
    package_id string:128;
    version bexos.version.SemVer;
    policy bexos.version.MultiVersionPolicy;
    is_active bool;
    disk_usage_bytes uint64;
};

@discoverable
protocol VersionManager {
    /// List all installed versions for a package
    ListVersions(struct {
        package_id string:128;
    }) -> (struct {
        status bexos.kernel.Status;
        versions vector<AppVersionEntry>:16;
    });

    /// Set active pinned version without downloading
    PinActiveVersion(struct {
        package_id string:128;
        target_version bexos.version.SemVer;
    }) -> (struct {
        status bexos.kernel.Status;
    });

    /// Rollback immediately to previous verified version
    RollbackToPrevious(struct {
        package_id string:128;
    }) -> (struct {
        status bexos.kernel.Status;
        restored_version bexos.version.SemVer;
    });

    /// Garbage-collect inactive versions older than a specific retention policy
    PruneInactiveVersions(struct {
        package_id string:128;
        keep_last_n uint32; // e.g., keep 2 versions for fast rollback
    }) -> (struct {
        status bexos.kernel.Status;
        reclaimed_bytes uint64;
    });
};

```

## Key Invariants

* **Instantaneous Zero-Download Rollbacks:** Reverting a broken app update does not touch the network; it modifies the active pin in `pinned_apps.redb` ($< 2\text{ ms}$).
* **Immutable Artifacts:** Package binaries (`.bex`) are content-addressed and read-only; new versions never overwrite old version blocks on disk.
* **Storage Pruning Guard:** Garbage collection will never delete the designated fallback version (`rollback_target_version`) if an app is still in a probation state.
