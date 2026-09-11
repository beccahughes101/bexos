# RFC 0030: Background job scheduling

- Created: 2026-08-30T10:28:21-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

Jobd coordinates declared background tasks with appd, power policy, and system and user stores. Job execution uses scoped capabilities, resource limits, and explicit lifecycle rules.

## Design overview

Scheduled background tasks in BexOS should be managed by a dedicated user-space daemon, **`jobd`** (or `scheduler`), integrated directly with `appd`'s process lifecycle, power management states, and dual `redb` stores.

## Implementation status

As of the current tree, the core jobs subsystem is implemented in the shipped
QEMU product:

- `jobd` exposes `bexos.job.Scheduler` ordinals 1-4 publicly; diagnostic
  `RunDueJobsNow` is reserved behind a system-privileged capability.
- App jobs are scheduled only for callers with service-binding package/UID
  identity and are clamped to decoded signed manifest declarations returned by
  appd's `WorkerLauncher.GetJobDeclarations`.
- Appd assigns each installed package a durable installation instance ID.
  `jobd` stores that ID with every job and purges jobs after uninstall or
  instance mismatch; package updates therefore require rescheduling.
- UID 0 durable jobs are stored at
  `/data/system/bexos.service.jobd/jobs.redb`; nonzero UID durable jobs are
  stored at `/data/users/<uid>/jobs.redb` only while usersd reports the user
  unlocked. Transient jobs stay in memory and heart-transplant migration state
  and are dropped on cold boot.
- Durable jobs use realtime UTC after timed reports synced/manual time.
  Durable jobs scheduled before a realtime anchor wait in an explicit
  `WAITING_REALTIME_ANCHOR` timebase and are anchored when time quality becomes
  valid. Transient jobs use monotonic time.
- Powerd, netstackd, timed, usersd, and appd expose watcher endpoints for power
  snapshots, link state, time quality, user state, and package policy changes.
  The watcher channels are preserved through each service's heart transplant.
- `jobd` consumes those watchers, fails closed when provider state is
  unavailable, marks due-but-blocked jobs as `WAITING_CONSTRAINTS`, marks
  locked-user jobs distinctly as `LOCKED_USER`, batches eligible flex-window
  jobs under one wake lease, and keeps the lease until each worker completes,
  fails launch, or times out.
- Worker launches flow through appd. `jobd` records the job token, appd records
  it in launch state, and completion/timeout/launch-failure paths call
  `WorkerLauncher.StopWorker`. Appd shares the kernel privileged
  `TerminateProcess` cleanup path so stopped workers exit all threads, release
  scheduler/futex state, signal termination, close owned handles, and release
  charged resources.
- Jobd migration records include scheduler clients, queued/suspended job state,
  running worker-control channels, wake leases, declaration cache,
  package-instance metadata, active batch state, provider watcher channels, and
  store routing. Stores are reopened on the target and durable state is merged
  with the quiesced in-memory snapshot.

Nonzero-UID job stores live inside encrypted per-user DiskImage homes. Usersd
unwraps the user's U-KEK, passes it to vfsd in a short-lived VMO, and vfsd
mounts the user's encrypted image before jobd opens `/data/users/<uid>/jobs.redb`.
On lock, jobd suspends that user's jobs and drops access to the encrypted store;
on unlock it reopens and merges the durable records from the remounted home.

Instead of running a traditional UNIX-like `cron` daemon that relies on arbitrary shell scripts and always-on CPU timers, BexOS should use an **intent-driven, condition-aware job scheduler** (similar to Android's `WorkManager` / `JobScheduler` and Apple's `BackgroundTasks` / `launchd`).

## Key Architectural Principles

* **No Constant Wake Locks:** Apps must never sit idling in memory just to wait for a timer. `appd` terminates or freezes idle apps; `jobd` wakes them up or launches a designated background worker component when the task triggers.
* **Condition & Power-Aware Batching:** Jobs define execution constraints (e.g., `REQUIRE_UNMETERED_NET`, `REQUIRE_CHARGING`, `REQUIRE_DEVICE_IDLE`). `jobd` coalesces wakes from multiple apps into unified execution windows to minimize CPU power-state transitions.
* **Dual-Store Scoping:** System maintenance tasks live in `/data/system/bexos.service.jobd/jobs.redb`, while per-user app tasks live in `/data/users/<uid>/jobs.redb`.

## Job Declaration in App Manifest (`manifest.proto`)

Apps declare background tasks statically in their package manifest, enabling `appd` to validate authority and resource constraints ahead of time:

```protobuf
syntax = "proto3";

package bexos.manifest;

enum JobNetworkConstraint {
  ANY = 0;
  UNMETERED_ONLY = 1; // Wi-Fi / Ethernet
  NONE = 2;           // Offline work
}

message JobDefinition {
  string job_id = 1;                  // e.g. "com.monzo.sync:transactions"
  string target_component = 2;        // Named runner/worker component in .bex

  // Scheduling parameters
  uint64 interval_seconds = 3;        // e.g., 3600 (Periodic)
  uint64 flex_window_seconds = 4;     // e.g., 300 (+/- 5m drift for batching)

  // Execution constraints
  JobNetworkConstraint network = 5;
  bool requires_charging = 6;
  bool requires_device_idle = 7;
  bool persist_across_reboots = 8;

  // Execution budget
  uint32 max_execution_seconds = 9;   // Hard timeout before appd kills worker
}

```

## Dual `redb` Job Storage

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ 1. SYSTEM JOB STORE (/data/system/bexos.service.jobd/jobs.redb)             │
├─────────────────────────────────────────────────────────────────────────────┤
│ • Managed by `jobd` for system tasks and hardware maintenance.             │
│ • Examples:                                                                 │
│   - `updated.check_tuf`: Every 6h (Unmetered Net)                     │
│   - `rosefs.trim_and_compact`: Daily (Device Idle + Charging)              │
│   - `domain_associations.refresh`: Weekly                                  │
└─────────────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────────────┐
│ 2. USER JOB STORE (/data/users/<uid>/jobs.redb)                             │
├─────────────────────────────────────────────────────────────────────────────┤
│ • Stored inside the encrypted home vault (active only when logged in).      │
│ • Examples:                                                                 │
│   - `com.monzo:monzo:sync`: Every 1h (Any Net)                              │
│   - `com.waymo:rider:telemetry_flush`: On Charging                          │
│ • Automatically purged when user uninstalls the app or logs out.            │
└─────────────────────────────────────────────────────────────────────────────┘

```

## Runtime Lifecycle & Job Execution Flow

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ 1. Timer / Hardware Alarm / Condition Met (e.g., AC power connected)        │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ 2. `jobd` (Scheduler Daemon)                                                │
│    • Evaluates pending jobs in `jobs.redb` matching active constraints      │
│    • Checks battery level, network type, and thermal throttle limits        │
│    • Batches 5 tasks into a single wakeup window                            │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │
                                       ▼ `appd.SpawnWorker(package_id, process, uid, job_token, job_control)`
┌─────────────────────────────────────────────────────────────────────────────┐
│ 3. `appd` (Application Daemon)                                              │
│    • Spawns the worker WASM/native process with minimal sandbox rights      │
│    • Passes ephemeral `JobControl` channel handle to worker startup args    │
│    • Starts execution watchdog timer (e.g., 30-second budget)               │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ 4. Worker Process Execution                                                 │
│    • Performs background work (e.g., fetches data over `netstack`)          │
│    • Calls `JobControl.Complete(Status::OK)`                                │
│    • `appd` terminates or suspends worker; `jobd` updates next run in redb  │
└─────────────────────────────────────────────────────────────────────────────┘

```

## `bexos.job.Scheduler` FIDL Protocol

```fidl
library bexos.job;

using bexos.kernel;

type NetworkRequirement = strict enum : uint8 {
    NONE            = 1;
    ANY             = 2;
    UNMETERED       = 3;
};

type JobConstraints = table {
    1: network NetworkRequirement;
    2: require_charging bool;
    3: require_device_idle bool;
    4: require_battery_not_low bool;
};

type JobSpec = struct {
    job_id string:64;
    initial_delay_seconds uint64;
    interval_seconds uint64;        // 0 for one-shot jobs
    flex_window_seconds uint32;     // Coalescing tolerance
    constraints JobConstraints;
    max_execution_seconds uint32;   // Execution budget (default: 30s)
};

@discoverable
protocol Scheduler {
    /// Schedule or update a job (Must match or be subset of app manifest declaration)
    ScheduleJob(struct {
        spec JobSpec;
    }) -> (struct {
        status bexos.kernel.Status;
    });

    /// Cancel a scheduled job
    CancelJob(struct {
        job_id string:64;
    }) -> (struct {
        status bexos.kernel.Status;
    });

};

@discoverable
protocol JobControl {
    /// Appd injects this worker-only channel; the worker reports completion to jobd.
    Complete(struct {
        token uint64;
        status JobRunStatus;
    }) -> (struct {
        status bexos.kernel.Status;
    });
};

```

## Safety and Security Invariants

* **No Arbitrary Command Execution:** Unlike classic `crontab`, jobs cannot execute arbitrary shell strings. They can only trigger explicitly declared, signed component entrypoints inside their `.bex` container.
* **Manifest Capability Clamping:** An app cannot dynamically request constraints or intervals that contradict its statically signed manifest (e.g., an app cannot schedule a 1-second infinite loop if its manifest allows only periodic 1-hour syncs).
* **Hard Resource Budgets:** Workers are bounded by CPU time and memory budgets. If a job exceeds its declared execution window (e.g., 30s), `appd` forcefully terminates the worker sandbox to prevent battery drain.
