# RFC 0031: Timekeeping and synchronization

- Created: 2026-08-30T10:29:33-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

Kernel monotonic clocks and realtime transforms provide time primitives; timed handles synchronization and RTC coordination. Shared time-page reads keep common clock access in userspace.

## Design overview

Time management in BexOS separates hardware monotonicity, kernel time primitives, and network synchronization into distinct architectural layers.

The current implemented milestone is deliberately smaller than the full design:
`timed` is a storage-installed userspace service, synchronizes with SNTPv4 or
NTS-selected network time through `netstack`, exposes `bexos.time.TimeManager`,
persists its target quality record in service `/data`, adjusts the kernel's
realtime transform through a narrow `SET_TIME` privileged method, uses bounded
500 ppm realtime slewing for small subsequent network corrections, and can
bootstrap/write back UTC through the QEMU PL031 RTC provider. The kernel now
exports the `TimePageV1` read-only VMO used by libc fast-time reads. Future
physical hardware work still includes board-specific RTCs, deeper boottime
sleep accounting, audit events, and richer clock-source selection.

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ 1. HARDWARE LAYER (ARM Generic Timer / x86 TSC + RTC Chip)                  │
│    • Unstoppable, monotonic hardware counter ticking at fixed frequency     │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ Raw Cycles
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ 2. D0 MICROKERNEL (Time Primitives & Transformation)                        │
│    • CLOCK_MONOTONIC: Strictly linear nanoseconds since boot                │
│    • CLOCK_BOOTTIME: Monotonic nanoseconds including low-power sleep states │
│    • Exposes user-space VMO with lock-free atomic scaling parameters        │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ Fast-path reading via VDSO / Shared VMO
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ 3. USERSPACE NETWORK TIME SERVICE (`timed` / SNTP and NTS)                 │
│    • Synchronizes over UDP with SNTP servers via `netstack`                 │
│    • Computes clock offset, frequency error, and root dispersion            │
│    • Applies gradual frequency slewing (adjtime) via D0 syscall             │
│    • Persists clock quality and latest epoch in `/system/state/time.redb`   │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ FIDL Interface
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ 4. APPLICATIONS & SERVICES (POSIX libc, UI, Job Scheduler)                  │
│    • Direct VDSO time reads (~5ns, no syscall overhead)                     │
│    • Wall-clock UTC time derived: `monotonic_now + utc_offset + slew_delta` │
│    • Timezone and locale conversions applied strictly in user-space         │
└─────────────────────────────────────────────────────────────────────────────┘

```

## The Clock Domains

BexOS supports three primary clock domains:

* **`CLOCK_MONOTONIC`:** Nanoseconds measured directly from the CPU hardware counter (ARM `CNTVCT_EL0` / x86 `RDTSC`). It never jumps backward, never steps, and ignores NTP adjustments. Used for timeouts, benchmarks, and IPC deadlines.
* **`CLOCK_BOOTTIME`:** Monotonic clock that continues incrementing across system deep-sleep and suspend cycles using the hardware Real-Time Clock (RTC).
* **`CLOCK_REALTIME` (Wall Clock / UTC):** Coordinated Universal Time represented as POSIX nanoseconds since January 1, 1970. This clock is adjusted by `timed` to match international time standards.

## Lock-Free Fast Time Reads (VDSO / Shared VMO)

To prevent applications from incurring syscall context-switch overhead every time they read the clock, D0 maintains a single, read-only system-wide **Time VMO** mapped into all user-space address spaces:

```rust
#[repr(C, align(64))]
pub struct SystemTimeValues {
    pub seq_lock: core::sync::atomic::AtomicU32,

    // Monotonic transformation: (ticks * multiplier) >> shift
    pub base_ticks: u64,
    pub base_monotonic_ns: u64,
    pub tick_multiplier: u32,
    pub tick_shift: u8,

    // Realtime transformation: monotonic_ns + offset_ns + active_slew
    pub utc_offset_ns: i64,
    pub slew_rate_ppm: i32, // Parts per million adjustment
    pub slew_remaining_ns: i64,
}

```

Applications read the hardware tick register directly in user-space and apply the transformation formula, taking **$< 5\text{ ns}$** per read without entering the microkernel.

## The `timed` Service (SNTP And NTS)

`timed` runs as a storage-installed userspace service after `netstack`:

* **Current Protocol Support:** Uses SNTPv4 via the `sntpc` crate and accepts
  NTS configuration through `SetTimeServers`. NTS-selected sync uses shared
  rustls/root handling from `//lib/net:net_secure`, performs NTS-KE with ALPN
  `ntske/1`, negotiates NTPv4 plus AES-SIV-CMAC-256, validates authenticated
  NTP extension framing, rotates returned cookies, and records `NTS_SECURE` as
  the source only after validation succeeds.
* **Current Kernel Adjustment:** Updates the kernel realtime offset through a
  `SET_TIME`-gated privileged clock-adjust method. Monotonic and boottime
  remain immutable.
* **Current Persistence:** Stores the latest `TimeQuality` record and UTC
  offset in `timed`'s service `/data` namespace.
* **Current Slewing and RTC:** Initial, failed, and manual corrections step
  realtime immediately; subsequent accepted network corrections under the
  configured threshold slew through the kernel at up to 500 ppm. On QEMU, the
  PL031 RTC driver provides `RtcHardware` bootstrap and best-effort writeback.
  Audit events and additional physical-hardware RTC implementations remain
  future work.

## `bexos.time` FIDL Interface

```fidl
library bexos.time;

using bexos.kernel;

type ClockSource = strict enum : uint8 {
    RTC_HARDWARE = 1;
    NTP_NETWORK  = 2;
    NTS_SECURE   = 3;
    CELLULAR_NITZ= 4;
    MANUAL_USER  = 5;
};

type TimeQuality = struct {
    source ClockSource;
    stratum uint8;
    root_dispersion_ns uint64;
    last_synced_timestamp uint64;
};

@discoverable
protocol TimeManager {
    /// Retrieve current synchronization status and time source
    GetTimeQuality() -> (struct {
        status bexos.kernel.Status;
        quality TimeQuality;
    });

    /// Trigger immediate network synchronization
    ForceSync() -> (struct {
        status bexos.kernel.Status;
    });

    /// Set manual wall-clock time (disables automatic network sync)
    SetManualTime(struct {
        utc_timestamp_ns int64;
    }) -> (struct {
        status bexos.kernel.Status;
    });

    /// Configure NTP/NTS pool servers
    SetTimeServers(struct {
        servers vector<string:128>:8;
        use_nts bool;
    }) -> (struct {
        status bexos.kernel.Status;
    });
};

```

## D0 Kernel Control Syscall

Only `timed` (granted `bexos.permission.SET_TIME` authority) can issue the privileged system call that adjusts the kernel transformation parameters:

```rust
// Privilege-checked syscall inside D0
pub fn sys_clock_adjust(
    clock_id: u32,
    offset_delta_ns: i64,
    slew_rate_ppm: i32,
) -> Result<(), KernelError>;

```

## Key Invariants

* **Kernel Never Touches the Network:** D0 has no concept of IP packets or NTP protocol structures; it only processes scalar frequency slews and offsets sent by `timed`.
* **Timezone Isolation:** The kernel and `timed` operate strictly in UTC nanoseconds. Timezones, daylight saving offsets, and formatting are resolved exclusively in application-level libraries using the Olson tzdata format stored on the filesystem.
* **Hardware RTC Sync:** On clean system shutdowns or successful slewing milestones, `timed` updates the battery-backed hardware RTC chip to keep cold boots accurate.
