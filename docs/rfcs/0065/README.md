# RFC-0065: Time Architecture, Immutable `tzdata` Packages, and Zero-Copy VMO Distribution

* **Author:** BexOS Core Platform & Internationalization Working Group
* **Status:** Proposed
* **Target Subsystems:** `timed`, `pkgd`, `prefsd`, `appd`, `lib/userspace/time`
* **Applicability:** Microkernel Time Abstractions, Runtime SDKs, Native Apps

---

## 1. Summary

This RFC defines the timekeeping and timezone architecture for BexOS. It establishes:

1. **Strict Kernel UTC Separation:** The D0 microkernel and RTC hardware operate exclusively on monotonic and UTC nanosecond ticks (`zx_time_t`), devoid of timezone, daylight saving time (DST), or leap-second policy.
2. **Immutable Binary `tzdata` Packaging:** Timezone definitions are compiled by Bazel into a flat, memory-mappable binary image (`tzdata.bextz`) distributed as a TUF-verified OCI artifact.
3. **Zero-Copy Memory Distribution:** Applications access timezone rules via duplicate read-only Virtual Memory Objects (`zx.Handle:VMO`), eliminating ambient filesystem traversal (`/usr/share/zoneinfo`).
4. **Live In-Place Updates:** Dynamic timezone database updates (e.g., geopolitical DST shifts) and user preference adjustments propagate across running processes without process restarts.

---

## 2. Motivation

Traditional Unix and Linux systems handle timezones through loose filesystem structures:

* **Ambient Filesystem Crawling:** Applications expect direct read access to `/usr/share/zoneinfo/` or `/etc/localtime`. In a capability-based microkernel with sandboxed D2 application domains, granting arbitrary directory traversal to read timezone files violates the principle of least privilege.
* **Redundant Parsing & I/O Overhead:** Each process independently discovers, opens, reads, and parses separate binary Olson files, causing redundant disk seeks, repeated heap allocations, and memory duplication across hundreds of sandboxed processes.
* **Geopolitical Volatility vs. System Upgrades:** Geopolitical entities frequently adjust daylight saving transitions or standard offsets on short notice. Coupling timezone databases to full operating system image updates or requiring system reboots risks scheduling and alarm errors.

BexOS separates monotonic physical time from localized presentation, packaging timezone data as an immutable, shared memory artifact resolved via `timed` and `prefsd`.

---

## 3. Detailed Architecture

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ APPLICATION RUNTIME (Dioxus Native / Rust / WASM)                           │
│                                                                             │
│  [ lib/userspace/time ]                                                     │
│  • Holds R/O mapped pointer to shared `tzdata.bextz`                        │
│  • Reads current active zone from Startup config / prefs: "America/Chicago"  │
│  • Zero-syscall local offset resolution & string formatting                 │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ Startup Handle / FIDL Protocol
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ `timed` (D1 Time & Synchronization Daemon)                                  │
│                                                                             │
│  ├── [ NTP/NTS Client ] ──► Queries networkd ──► Disciplines UTC clock      │
│  │                                                                          │
│  ├── [ VMO Provider ] ◄── Holds active `tzdata.bextz` handle                │
│  │                        (BootFS baseline or TUF update from `pkgd`)       │
│  │                                                                          │
│  └── [ Timezone Change Broadcaster ] ──► Notifies active UI applications    │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ Duplicates Read-Only VMO Handle
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ PHYSICAL MEMORY (Shared Pages)                                              │
│                                                                             │
│  [ Immutable `tzdata.bextz` Image (~450 KB) ]                               │
│         ▲                                   ▲                               │
│         │ Mapped R/O into App A             │ Mapped R/O into App B         │
└─────────┴───────────────────────────────────┴───────────────────────────────┘

```

---

### 3.1 Kernel & RTC Boundary (Pure UTC)

The D0 microkernel maintains two clocks exposed through syscalls:

* `zx_clock_get_monotonic()`: Nanoseconds since boot, monotonic, non-adjustable.
* `zx_clock_get(ZX_CLOCK_UTC)`: Nanoseconds since the Unix epoch (1970-01-01T00:00:00Z).

`timed` synchronizes `ZX_CLOCK_UTC` via network time protocols (NTS/NTP) by calling `zx_clock_update()`. The microkernel has zero awareness of timezone identifiers, historical offsets, or leap second scheduling; all localized transformations are calculated exclusively in userspace.

---

### 3.2 Compilation & OCI Packaging (`tzdata.bextz`)

The upstream IANA Time Zone database source files are compiled by Bazel into a compact, single-binary index table:

```
//platform/data/zoneinfo/
├── BUILD.bazel
├── src/
│   ├── compile_tz.py          # Serializes IANA zone files into bextz layout
│   └── raw/ (northamerica, europe, australasia, etc.)

```

#### Binary Layout (`tzdata.bextz`)

```
+-----------------------------------------------------------------------+
| Header (Magic: "BEXTZ02", Version: "2026b", Zone Count: N)            |
+-----------------------------------------------------------------------+
| String Table (Null-terminated Zone Names, e.g., "America/Chicago")    |
+-----------------------------------------------------------------------+
| Zone Index (Sorted array of [Name Offset, Table Offset, Record Count])|
+-----------------------------------------------------------------------+
| Transition Records (Packed [Timestamp_UTC, Offset_Secs, DST_Flag])   |
+-----------------------------------------------------------------------+

```

The compiled output is packaged as an OCI artifact (`bexos.platform.tzdata`) signed with TUF metadata:

* **System Baseline:** Stored at `/system/data/zoneinfo/tzdata.bextz` inside the immutable system image.
* **Registry Overrides:** Updated versions are pushed to `pkg.bexos.org/platform/tzdata:<version>`. When published, `pkgd` downloads and verifies the payload against the hardware rollback counter in `trusty`.

---

### 3.3 Zero-Copy Runtime Delivery Flow

Applications acquire timezone access through startup handles without requiring network or filesystem permissions:

1. **Process Launch:**
* When `appd` spawns a process, it requests an active `tzdata` VMO handle from `timed`.
* `timed` duplicates its cached handle with attenuated permissions:
```rust
let client_vmo = cached_tzdata_vmo.duplicate_handle(
    zx::Rights::READ | zx::Rights::MAP | zx::Rights::GET_PROPERTY
)?;

```


* `appd` injects `client_vmo` into the process startup table under the handle tag `PA_TZDATA_VMO`.
* `appd` includes the active timezone preference string (e.g., `timezone: "America/Chicago"`) in the compiled `BEXCFG` configuration table.


2. **In-Memory Resolution (`lib/userspace/time`):**
* The client runtime memory-maps the VMO:
```rust
let base_addr = zx_vmar_map(
    vmar_root,
    zx::VmOption::PERM_READ,
    0,
    tzdata_vmo.raw_handle(),
    0,
    vmo_len,
)?;

```


* A localized lookup (e.g., formatting `2026-09-13T14:01:06Z` for the local zone) binary-searches the mapped zone index, walks the transition table, and extracts the UTC offset in nanoseconds.
* The operation executes entirely in memory with **zero system calls and zero heap allocations**.



---

### 3.4 Live Dynamic Updates

#### Scenario A: The User Changes Timezones

1. The user selects a new timezone via `sysui` or Settings.
2. `prefsd` writes the preference to the user's encrypted volume (`/data/users/<uid>/prefs/`) and triggers an `OnConfigChanged` event.
3. The running application's configuration observer receives the event and updates its local active timezone pointer atomically.
4. Next-frame UI renders reflect the new timezone immediately without restarting the application.

#### Scenario B: Geopolitical DST / Rule Updates

1. `pkgd` detects a newly signed `bexos.platform.tzdata` release on the OCI registry.
2. `pkgd` verifies the artifact's TUF signature and checks version monotonicity against `trusty` RPMB storage.
3. `timed` ingests the new layer blob, validates the `BEXTZ02` header, and caches the new VMO handle.
4. `timed` broadcasts the updated VMO across the system notification bus:

```fidl
library bexos.time;

using bexos.kernel;

@discoverable
protocol TimezoneNotificationListener {
    /// Dispatched when the underlying tzdata binary database is updated
    OnTimezoneDatabaseUpdated(resource struct {
        tzdata_vmo zx.Handle:VMO;
        version string:16;
    }) -> ();
};

```

5. Client runtimes receive the new VMO, remap their internal data structures to the updated memory pages, and release the previous mapping.

---

## 4. Security & Fault Isolation

| Vector | Mitigation Strategy |
| --- | --- |
| **Sandboxed Client Exploitation** | Client applications are never granted filesystem read rights to arbitrary directories. They receive an explicit read-only VMO handle stripped of `WRITE` and `EXECUTE` rights. |
| **Malicious Registry Metadata** | `pkgd` validates the signature of updated `tzdata` artifacts against offline TUF root keys before passing the memory buffer to `timed`. |
| **Memory Exhaustion via Bloated Tables** | `timed` validates the total byte length and internal table offsets of `tzdata.bextz` before caching or broadcasting the handle, rejecting oversized or malformed payloads. |
| **Tampering Between Processes** | Because the VMO is mapped with `PERM_READ`, any attempt by a compromised application to alter transition offsets causes an immediate hardware page fault (`SIGSEGV`), terminating only the misbehaving client. |

---

## 5. Implementation Plan

### Phase 1: Build Rules & Binary Layout

* Implement `//platform/data/zoneinfo:compile_tz` rule generating `tzdata.bextz` from upstream IANA source files.
* Implement `lib/userspace/time` parsing routines capable of resolving UTC offsets against an in-memory mapped buffer.

### Phase 2: `timed` VMO Management & Process Integration

* Integrate `tzdata.bextz` into the base system image at `/system/data/zoneinfo/tzdata.bextz`.
* Update `timed` to load this baseline into a persistent read-only VMO and implement handle duplication for `appd`.
* Update `appd` to pass `PA_TZDATA_VMO` and user preference configuration to newly launched processes.

### Phase 3: Dynamic Updates via `pkgd`

* Define the `bexos.platform.tzdata` OCI artifact schema.
* Connect `timed` to `pkgd` via `libpkg_client` to support automated staging of new timezone databases.
* Implement the `TimezoneNotificationListener` broadcast protocol in the Rust userspace SDK.