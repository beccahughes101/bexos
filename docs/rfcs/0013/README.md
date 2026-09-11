# RFC 0013: Power management

- Created: 2026-08-26T21:38:15-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

The kernel provides hardware power mechanisms; powerd owns policy, wake leases, and coordinated device transitions. The design separates the implemented QEMU path from future board and suspend support.

## Design overview

Power management separates hardware mechanisms from policy, as timekeeping
does. The kernel owns raw hardware execution states and hardware monitors;
the userspace `powerd` service owns policy, coordination, and device
transitions.

The implemented v1 path includes the generated `bexos.power` FIDL surface,
wake leases, D1 device power-control hooks, a booted `powerd` service,
and a privileged kernel handoff call. On the current `qemu_aarch64` raw-kernel
boot path, QEMU provides PSCI through the SMC conduit. The maintained QEMU
AArch64 product backs `SUSPEND_TO_RAM` with PSCI `CPU_SUSPEND` standby until
an interrupt, and backs reboot/poweroff with PSCI `SYSTEM_RESET`/`SYSTEM_OFF`;
future boards that provide a fuller firmware contract can back the same kernel
primitive with `SYSTEM_SUSPEND` or ACPI system sleep.

## The Power Architecture: Split Responsibility

```
┌─────────────────────────────────────────────────────────────┐
│ USERSPACE: Power Manager (`bexos.power.PowerManager`)       │
│ • High-level policy: Battery thresholds, thermal throttling │
│ • System transitions: Suspend-to-RAM (S3/s2idle), Hibernate │
│ • Wake locks / Power leases held by apps and services       │
│ • Orchestrates multi-driver device power states (D0–D3)     │
└──────────────────────────────┬──────────────────────────────┘
                               │ FIDL Power Controls
                               ▼
┌─────────────────────────────────────────────────────────────┐
│ KERNEL: Raw Hardware Power Primitives                       │
│ • CPU Idle C-states (WFI / UMWAIT / MWAIT) when runqueue is │
│   empty                                                     │
│ • CPU Frequency & P-states (DVFS / energy-performance bias) │
│ • Hardware sleep entry: flushing TLBs, parking secondary    │
│   cores, executing low-level firmware power hooks (PSCI/ACPI│
└─────────────────────────────────────────────────────────────┘

```

## What Stays in the Kernel (Mechanism Only)

The microkernel handles low-level hardware transitions that cannot tolerate userspace IPC latency:

### Idle Loop (C-States)

When a CPU core's scheduler runqueue is empty, the kernel immediately places the core into a low-power idle state (`WFI` on ARM, `MWAIT`/`TPAUSE` on x86) until the next hardware timer or interrupt fires.

### CPU Frequency Scaling (P-States / DVFS)

The kernel provides low-level hooks to set performance registers (e.g., ARM `CPPC` / Intel HWP / AMD CPPC) based on current runqueue utilization metrics.

### Hardware Suspend, Reboot, and Poweroff

`SystemPrivileged.RequestSystemPowerState` is the privileged handoff point for
whole-system transitions. `SUSPEND_TO_RAM`, `REBOOT`, and `POWEROFF` are the
supported v1 states. `SUSPEND_TO_DISK` returns `ERR_UNSUPPORTED` until BexOS
has a hibernation image format and storage resume policy.

During a firmware-backed suspend, the kernel sequence is:
1. Parks secondary CPU cores.
2. Saves CPU register state and architecture-specific control registers.
3. Flushes local CPU state as required by the platform.
4. Enters firmware sleep mode, such as PSCI `SYSTEM_SUSPEND` or ACPI
   `S3`/Modern Standby.

The QEMU raw `-kernel` path uses QEMU's built-in PSCI service rather than a
physical board firmware stack. Because QEMU's fake PSCI does not implement
`SYSTEM_SUSPEND`, the current product's `SUSPEND_TO_RAM` request is CPU
standby through `CPU_SUSPEND` until an interrupt. Reboot and poweroff use PSCI
reset and shutdown.

## What Runs in Userspace (`bexos.power.PowerManager`)

All decision-making, heuristics, and device-level power coordination live in an unprivileged userspace service:

### Wake Leases (Power Locks)

Applications and background daemons request power leases (e.g., "keep CPU awake
while downloading package" or "keep display active while playing video"). The
power manager blocks suspend-to-RAM as long as valid lease handles remain open.
The lease ends when the returned handle is closed.

### Driver Device Power States (D0–D3)

When transitioning into system sleep, `powerd` coordinates with D1
drivers over `DevicePowerControl`:
1. Signals the NVMe driver to flush volatile caches and drop to low-power `D3hot`.
2. Signals the GPU and display engine to power down rails and panel backlights.
3. Signals the PCIe root bus to transition links to low-power states (L1/L2).

### Thermal & Battery Policy

Current policy polls optional telemetry providers once per second, reports
provider absence as unavailable, throttles on low battery or thermal pressure,
retains safer throttles while a previously healthy provider is stale, and
applies reduced/minimum caps to the built-in background `ResourceGroup`.
Optional DVFS providers receive the selected nominal/reduced/minimum level.
QEMU currently lacks battery, thermal, and DVFS hardware providers, so the
generic machinery is operational without fabricated measurements.

## Power Management FIDL Interface

```fidl
library bexos.power;

type SystemPowerState = strict enum : uint8 {
    ACTIVE          = 1;
    SUSPEND_TO_RAM  = 2;
    SUSPEND_TO_DISK = 3;
    REBOOT          = 4;
    POWEROFF        = 5;
};

type DevicePowerState = strict enum : uint8 {
    D0_FULL_POWER = 0;
    D1_STANDBY    = 1;
    D2_LOW_POWER  = 2;
    D3_OFF        = 3;
};

/// Implemented by D1 device drivers to handle power transitions
protocol DevicePowerControl {
    SetPowerState(struct { state DevicePowerState }) -> (struct {
        status Status
    });
};

/// Public service exposed by PowerManager to applications and daemons
@discoverable
protocol PowerManager {
    /// Acquire a lease to keep the system awake (lease ends when handle is dropped)
    AcquireWakeLease(struct { reason string:64 }) -> (resource struct {
        status Status,
        lease_handle handle:EVENT
    });

    /// Request a whole-system state transition (gated by privileged capability)
    RequestSystemState(struct { state SystemPowerState }) -> (struct {
        status Status
    });
};

```

## Suspend Lifecycle Walkthrough

```
[ PowerManager ]                [ D1 Drivers (NVMe/GPU) ]          [ Microkernel ]
       │                                   │                              │
       ├─ 1. Evaluates wake leases (none) ─┤                              │
       │                                   │                              │
       ├─ 2. SetPowerState(D3) ───────────>│                              │
       │<─ 3. Flushed & Acked ─────────────┤                              │
       │                                                                  │
       ├─ 4. RequestSystemPowerState(SUSPEND_TO_RAM) ────────────────────>│
                                                                          │ (Freezes scheduler)
                                                                          │ (Parks secondary cores)
                                                                          │ (Executes PSCI/ACPI sleep)

```

1. **Lease Evaluation:** `PowerManager` ensures all wake leases have been released or expired.
2. **Driver Quiescing:** Over FIDL, all D1 drivers flush in-flight DMA operations and transition hardware into low-power states.
3. **Kernel Handover:** `powerd` calls the privileged kernel system
   power interface, allowing the microkernel to execute the minimal
   hardware-level transition or return an explicit unsupported status.
