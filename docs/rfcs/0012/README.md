# RFC 0012: Kernel scheduling

- Created: 2026-08-26T21:30:07-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

BexOS combines fixed-priority fair scheduling, realtime execution, capability-gated profiles, and tickless SMP operation. Resource groups account for execution and define future policy extensions.

## Design overview

BexOS supports time-critical userspace drivers, display/input services, and
general-purpose applications. The scheduler is evolving toward a full realtime
microkernel architecture. The current QEMU implementation uses a config-driven
SMP topology with per-CPU current tasks, affinity-aware selection, tickless
generic-timer deadlines, and reschedule SGIs. The design below retains future
policy extensions, not a second host-only scheduler model.

The implemented v1 scheduler supports:

* Fixed-priority fair scheduling with 256 priority levels.
* Bitmap-based lookup for the highest runnable fair-priority band.
* Round-robin rotation among runnable tasks at the same fair priority.
* EDF realtime profiles with single-CPU admission control.
* Capability-backed scheduling profile handles.
* Directed yield to donate the current scheduling opportunity to a ready thread.
* Configured CPU affinity masks and per-CPU current task slots.
* Futex priority inheritance when the waiter supplies the owner thread handle.
* Synchronous channel call/read/reply tokens with policy-controlled priority
  donation while a server handles a call.
* Resource-group CPU-share enforcement for fair-scheduled work, anonymous VMO
  memory charging, and GPU reservation accounting.

The remaining design work is broader hardware validation and a real GPU/display
stack; GPU resource limits are currently reservation accounting only.

## Current Scheduler Domains

```
+=========================================================================+
|                       BexOS SMP Scheduler Domains                       |
+=========================================================================+

  [ Domain 0: Realtime Deadline / EDF ]
  * D1 driver loops, audio buffers, compositor-critical work
  * Model: SCHED_DEADLINE-style earliest absolute deadline first
  * Rule: admitted capacity (C) within period (T), deadline (D)
  * Preempts all fair-priority work
              |
              v
  [ Domain 1: Fixed-Priority Fair Work ]
  * System services, foreground apps, background jobs
  * Model: 256 fair priority levels, bitmap highest-priority lookup
  * Rule: highest priority runs first; equal priorities rotate round-robin
              |
              v
  [ Domain 2: Idle / Low Power ]
  * CPU idling and background maintenance when no task is runnable
```

Resource group ids are tracked as process/task metadata, exposed through debug
records, and enforced for fair-scheduled CPU time. Runnable deadline tasks still
run before all fair work. When only fair tasks are runnable, the scheduler first
chooses the runnable resource group with the lowest weighted virtual runtime,
then chooses the highest-priority ready task inside that group and rotates among
equal-priority tasks. A task can run only on CPUs included in its affinity mask,
and a running task is not selected by another CPU.

## Implemented Realtime Mechanisms

### A. Scheduling Profiles

Scheduling policy is attached through capability-backed profile handles. A
profile is minted by the kernel control plane and then applied to a thread.

Fair profiles:

```fidl
struct FairProfile {
    priority uint8;
    weight uint32;
};
```

Deadline profiles:

```fidl
struct DeadlineProfile {
    capacity_ns uint64; // Worst-case execution time (C)
    deadline_ns uint64; // Relative deadline from activation (D)
    period_ns uint64;   // Repetition period (T)
};
```

The local FIDL dialect exposes scheduling profiles as a strict union:

```fidl
type SchedulingProfileInfo = strict union {
    1: fair FairProfile;
    2: deadline DeadlineProfile;
};
```

### B. EDF Admission and Execution

Before a deadline profile is applied, the kernel validates:

* `capacity_ns > 0`
* `capacity_ns <= deadline_ns`
* `deadline_ns <= period_ns`
* total admitted utilization does not exceed one CPU in the current admission
  model

The utilization check is:

```
sum(capacity_ns / period_ns) <= 1.0
```

Runnable deadline tasks are selected before all fair-priority tasks. Among
deadline tasks, the scheduler chooses the task with the earliest absolute
deadline.

### C. Fixed Priority Fair Scheduling

Fair tasks have base and effective priorities in the range `1..=255`.
Priority `0` is invalid for ordinary runnable work. The scheduler keeps a
four-word bitmap covering all 256 priority levels and uses the highest set bit
to find the next runnable fair-priority band. Tasks at the same priority rotate
round-robin from the previously running task.

### D. Priority Inheritance

Futex priority inheritance is implemented for waits that name the contended
owner thread:

```fidl
FutexWait(struct {
    uaddr uint64,
    expected_val uint32,
    timeout_nanos int64,
    owner_thread handle:OPTIONAL
}) -> (struct { status Status });
```

When a high-priority waiter blocks on a futex owned by a lower-priority thread,
the owner receives a temporary effective-priority boost. The boost is recomputed
when waiters wake or exit, so remaining waiters continue to donate priority
until their waits resolve.

### E. Directed Yield / Timeslice Donation

Directed yield is exposed as:

```fidl
YieldThread(resource struct {
    target_thread handle:OPTIONAL
}) -> (struct { status Status });
```

When `target_thread` is absent, the current thread yields normally. When it is
present, the scheduler switches directly to that ready target thread on the
same CPU if the target is ready and eligible for that CPU's affinity mask. This
is the current explicit form of timeslice donation.

Synchronous `Call`/`ReadCall`/`ReplyCall` uses single-use reply-token handles.
When channel policy enables priority inheritance or timeslice donation, the
server thread inherits the caller's effective fair priority while it owns the
request and the donation unwinds when `ReplyCall` consumes the token. The
current donation implementation is bounded to fair-priority inheritance in the
host-testable control plane; full deadline-budget donation and nested-chain
cycle detection remain future hardening work.

### F. Channel Scheduling Policy Metadata

Channels can store scheduling policy flags:

```fidl
SetPolicy(resource struct {
    channel handle:CHANNEL,
    enable_priority_inheritance bool,
    enable_timeslice_donation bool
}) -> (struct { status Status });
```

The metadata is consumed by the synchronous call path. Asynchronous
`ReadMessage`/`WriteMessage` queue behavior is unchanged.

## Capability Gating

Realtime scheduling must not be ambient. In v1:

* General application processes can mint fair profiles up to priority `127`.
* Fair priorities above `127` and deadline profiles require the manifest
  permission `bexos.permission.REALTIME_SCHEDULING`.
* The appd launch path grants that runtime capability only when the package ID
  and expected signer also match the platform prototxt allowlist.
* Hardware-access tier does not grant scheduling privilege.

`HardwareAccess::Direct` is a temporary stand-in for manifest-backed realtime
permissions. The target permission remains:

```
bexos.permission.REALTIME_SCHEDULING
```

The host-testable `ControlPlane` still has older tests for hardware-access
profile gating; the QEMU launch path carries the manifest/allowlist result as
the process realtime bit used by the bare runtime.

## Kernel FIDL Surface

All current scheduler FIDL lives in the existing `bexos.kernel` library so it can
be generated by the current Bazel-backed FIDL pipeline.

Profile minting:

```fidl
protocol ProfileProvider {
    CreateProfile(struct {
        info SchedulingProfileInfo
    }) -> (resource struct {
        status Status,
        profile_handle handle:PROFILE
    });
};
```

Thread control additions:

```fidl
protocol TaskControl {
    SetProfile(resource struct {
        thread handle:THREAD,
        profile handle:PROFILE
    }) -> (struct { status Status });

    SetCpuAffinity(resource struct {
        thread handle:THREAD,
        affinity CpuMask
    }) -> (struct { status Status });

    YieldThread(resource struct {
        target_thread handle:OPTIONAL
    }) -> (struct { status Status });
};
```

CPU affinity is accepted for nonzero masks contained within the active CPU mask
from the configured topology. Masks that include absent CPUs return
`ERR_INVALID_ARGS`.

## SMP Design

The implemented SMP foundation uses per-CPU current task slots and affinity
filtering:

* Each CPU schedules from the shared task table.
* A task with state `Running` is owned by one CPU and cannot be selected by a
  sibling CPU.
* Affinity masks are `u64`, so the configured topology is capped at 64 CPUs.
* Idle CPUs may remain idle when no ready task is eligible for their CPU.

The runnable indexes are maintained per CPU through affinity and ownership;
the next optimization is making their storage physically separate rather than
the current shared interrupt-safe scheduler state:

* Each CPU owns its runnable EDF heap and fair-priority bitmap.
* Realtime and pinned tasks stay on their assigned CPU.
* Idle CPUs may steal fair-share work from other CPUs.
* Cross-CPU migration avoids realtime tasks unless a future affinity policy
  explicitly permits it.

## Tickless Timer Design

The kernel is tickless:

* Program the hardware timer for the current deadline budget expiration.
* Program the hardware timer for the earliest sleeping-thread wakeup.
* Avoid periodic timer interrupts when no scheduling event is due.

Scheduler accounting uses monotonic nanoseconds. Each CPU programs its absolute
generic-timer deadline for the running fair quantum, EDF budget, quota/window
event, or blocked timeout; an idle CPU with no event disables its local timer.

Execution accounting charges deadline budgets at monotonic syscall and scheduling
boundaries, including voluntary yields and blocking. Timer handling uses that
remaining budget without charging the same interval again. This prevents yielding
deadline tasks from retaining their reservations while starving fair tasks.

## Resource Group CPU Shares

Resource groups are hierarchical kernel records with a name, non-zero CPU
weight, structured CPU/memory/GPU limits, and compatibility decoding for the
legacy `cpu_shares` and `memory_limit_pages` fields. The built-in groups are:

* `system` (`1`) with `2048` shares
* `foreground` (`2`) with `1024` shares
* `background` (`3`) with `256` shares
* `driver` (`4`) with `1536` shares

Fair scheduling uses group-level weighted virtual runtime. A fair tick charges:

```
vruntime += 1024 * 1024 / cpu_shares
```

Higher-share groups accumulate virtual runtime more slowly and therefore receive
more fair CPU time when competing groups remain runnable. Inside the selected
group, existing priority and round-robin rules still apply.

EDF admission checks the selected CPU, every ancestor's realtime permission,
and CPU cap before accepting a deadline profile. Anonymous VMO allocations
charge the creator's leaf resource group and all ancestors, reject allocations
above any configured high watermark, and uncharge when the VMO is released.
GPU reservations are capability-backed, charge the selected group and its
ancestors atomically, and release on handle close; future GPU/display
components must use this reservation API as their enforcement boundary.

## Deferred Resource Group Design

The hierarchy below is the implemented capability container shape. Explicit
manifest parents are restricted to built-ins or groups in the same package;
missing parents, cycles, and conflicting legacy/structured limits are rejected.

Future topology:

```
+=============================================================================+
|                       Root Resource Domain (System)                         |
+=============================================================================+
        |
        +-- [ Domain Group: "system_critical" ] (Uncapped, Realtime Allowed)
        |     +-- D1 NVMe Driver (Thread Priority: 240, Uncapped DMA VMOs)
        |     +-- Display Compositor (EDF: C=4ms, T=8.33ms)
        |
        +-- [ Domain Group: "ui_foreground" ] (High Weight, Priority Band 100-180)
        |     +-- Active App: com.example.game
        |
        +-- [ Domain Group: "vendor:com.google:background" ]
              +-- Max 10% CPU, 512MB RAM, 0% Realtime
              +-- Process: com.google.drive
              +-- Process: com.google.photos
```

Future resource FIDL shape:

```fidl
library bexos.resource;

using bexos.kernel;
using bexos.scheduler;

type CpuThrottle = struct {
    max_utilization_permille uint16;
    weight uint32;
    allow_realtime bool;
};

type MemoryWatermarks = struct {
    low_watermark_bytes uint64;
    high_watermark_bytes uint64;
};

type GpuLimits = struct {
    max_render_budget_percent uint8;
    max_vram_bytes uint64;
};

type ResourceGroupLimits = struct {
    cpu CpuThrottle;
    memory MemoryWatermarks;
    gpu GpuLimits;
};

@discoverable
protocol ResourceGroupManager {
    CreateGroup(resource struct {
        name string:64,
        limits ResourceGroupLimits,
        parent handle:RESOURCE_GROUP?
    }) -> (resource struct {
        status bexos.kernel.Status,
        group_handle handle:RESOURCE_GROUP
    });

    SetLimits(resource struct {
        group handle:RESOURCE_GROUP,
        limits ResourceGroupLimits
    }) -> (struct { status bexos.kernel.Status });
};
```

Future scheduler integration:

* Profile creation is evaluated relative to the caller's resource group.
* EDF is rejected when `group.cpu.allow_realtime == false`.
* Fair priority is clamped to the group's allowed band.
* Group-level virtual runtime or quota accounting chooses the group first, then
  the thread within that group.

The listed admission and virtual-runtime rules are the active scheduler policy;
future GPU/display consumers must acquire a reservation before using GPU budget.
