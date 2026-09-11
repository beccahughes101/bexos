# RFC 0011: Kernel hardware features and memory regions

- Created: 2026-08-26T21:23:07-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

The kernel design covers CPU isolation, memory translation, scheduling primitives, hierarchical VMARs, and clock mechanisms, with policy and network time synchronization delegated to userspace.

## Design overview

A modern operating system kernel relies on several essential hardware CPU features to ensure stability, security, isolation, and performance.

## Memory Management & Address Translation

* **Paging & Hierarchical Page Tables:** Fundamental for virtual memory and per-process memory isolation (`CR3` on x86_64, `TTBR0/1_EL1` on ARM64, `satp` on RISC-V).
* **NX / XD (No-Execute / Execute-Disable):** Marks data pages (heap, stack) as non-executable to prevent code injection attacks (W^X / DEP).
* **PCID / ASID (Process-Context Identifiers / Address Space IDs):** Tags TLB entries with an ID per address space. This eliminates the need to flush the entire TLB on every context switch between processes.
* **Huge Pages (2MB / 1GB):** Crucial for reducing TLB miss penalties for kernel heaps, framebuffers, and memory-intensive processes.
* **Global Pages (PGE on x86):** Prevents common kernel memory mappings from being flushed out of the TLB during address space switches.

## Threading, Context Switching & State Management

* **Fast Syscalls (`syscall/sysret` on x86, `svc` on ARM64, `ecall` on RISC-V):** Low-overhead ring switches avoiding slow legacy software interrupts.
* **Per-CPU & Thread Pointers (`FSGSBASE` / `SWAPGS` on x86, `TPIDR_EL0/1` on ARM64):** Enables user-level Thread-Local Storage (TLS) and gives the kernel immediate, fast access to per-CPU data structures without memory lookups.
* **Floating Point & Vector Registers (`XSAVE` / `XRSTOR` / SSE / AVX / Neon):** Managing SIMD register contexts. Kernels typically use `XSAVEOPT` / `XSAVEC` to save only the registers that have actually been dirtied by user programs.

## CPU Security & Privilege Separation

* **SMEP / PXN (Supervisor Mode Execution Prevention):** Prevents the kernel from executing code residing in user-space pages, stopping ret2usr exploits.
* **SMAP / PAN (Supervisor Mode Access Prevention / Privileged Access Never):** Prevents the kernel from accidentally reading/writing user-space memory directly without an explicit unlock wrapper (like `copy_from_user`).

### Control-Flow Integrity (CET / BTI / PAC)

* **Shadow Stacks:** Hardware-enforced return address stacks to stop ROP (Return-Oriented Programming).
* **IBT / BTI / Pointer Authentication:** Restricts indirect jumps and function pointers to verified branch targets to stop JOP/COP attacks.

* **Speculation Barriers / Mitigations:** Managing speculative execution flags (IBPB, STIBP, SSBD) to mitigate Spectre/Meltdown-style side-channel attacks.

## Interrupts & Timer Management

* **Advanced Interrupt Controllers (x2APIC / ARM GICv3+ / RISC-V PLIC/AIA):** High-efficiency interrupt routing, inter-processor interrupts (IPIs), and MSI/MSI-X support for PCIe devices.
* **Deadline Timers & Monotonic Clocks (TSC Deadline / ARM Generic Timer):** Generates interrupts at precise cycle counts instead of relying on legacy periodic PIT/HPET ticks, saving CPU cycles and enabling tickless kernels.
* **Hardware Virtualization Interrupt Delivery (APIC-v / GIC ITS):** Directly posts interrupts to virtualized guests or isolated microVMs without kernel trapping.

## Power & Low-Latency Synchronization

* **User-Space Wait / Monitor (`UMWAIT` / `TPAUSE` / `WFE` / `WFI`):** Allows idle threads or spinlocks to put the CPU core into an energy-efficient low-power sleep state until an address changes or an interrupt arrives, rather than burning 100% CPU in a busy-spin loop.
* **Hardware Atomic Primitives (LSE Atomics on ARM, CAS/CMPXCHG on x86):** Native non-blocking synchronization instructions for lock-free queues, counters, and scheduler run queues.

## Implementation Priority Roadmap

| Tier | Features | Why Implement First |
| --- | --- | --- |
| **Tier 1 (Core)** | Paging, Fast Syscalls, Per-CPU registers (`SWAPGS`/`TPIDR`), Timers | Required to run multiple isolated processes with basic preemptive multitasking. |
| **Tier 2 (Stability & Perf)** | `XSAVE`/`XRSTOR`, PCID/ASID, Huge Pages, NX/XD bits | Keeps user applications (libc, math, SIMD) from crashing and speeds up context switches. |
| **Tier 3 (Hardening)** | SMEP/SMAP, Shadow Stacks (CET/PAC), Speculation controls | Protects kernel memory boundaries from malicious or buggy user code. |
| **Tier 4 (Advanced)** | Hardware Virtualization (VT-x/SVM/EL2), Dynamic feature patching (`alternatives`) | For running hypervisors, containers, or hardware-optimized binary code paths. |

## BexOS Implementation State

The current implementation is AArch64-first and covers roadmap Tiers 1-3 with first-class VMARs in the kernel service control plane.

* **Tier 1:** The kernel uses page tables, `svc`-based syscall dispatch, per-CPU `TPIDR_EL1`, preserved user `TPIDR_EL0`, and the ARM generic timer. `bexos.kernel.Clock` exposes monotonic and boot-time nanoseconds; wall-clock time is intentionally absent from the kernel.
* **Tier 2:** User mappings preserve W^X, data pages are UXN/PXN, executable user pages are read-only/PXN, FP/SIMD context is saved and restored in the trap frame, and process address spaces receive ASIDs when the CPU advertises ASID support. Huge-page descriptor helpers remain the kernel mapping foundation.
* **Tier 3:** PXN/WXN and PAN are enforced where supported. Native AArch64 product targets use the pinned nightly Rust branch-protection policy for BTI plus return-address PAC. The kernel distinguishes detected CPU features, compiled compatibility, entropy readiness, and active controls; PAC remains inactive and truthfully reported when BTI/PAC hardware or entropy is absent. Speculation barriers are selected per CPU, using `SB` where available and a `DSB`/`ISB` fallback otherwise, with SSBS configured when present.
* **VMAR state:** `bexos.kernel.VirtualMemory` exposes VMAR handles, nested sub-region creation, VMAR-relative VMO mapping, unmapping, and recursive non-root VMAR teardown. `CreateProcess` returns a restricted root-VMAR construction handle, and appd's production ELF loader uses hierarchical VMARs for image, library, TLS, stack, and guard regions. Legacy flat `Map`, `MapInVmSpace`, `Unmap`, and `UnmapInVmSpace` calls remain as compatibility wrappers over each VM space's root VMAR.
* **Explicit non-goals for this phase:** no wall-clock/UTC/calendar/timezone/RTC/NTP logic in ring 0. The kernel does expose the current monotonic/boottime/realtime transform through a read-only seqlock `TimePageV1` VMO for userspace vDSO-style fast paths.

**Virtual Memory Address Regions (VMARs)** organize complex address spaces hierarchically instead of exposing only flat page mappings. Capability microkernels such as Fuchsia’s Zircon use this model for safe address-space management.

**Status:** VMARs are implemented in `kernel/core::kernel_services` as allocation-backed control-plane records and in the active appd ELF runner. The flat VMO-to-address-space mapping API (`Map`, `MapInVmSpace`, `Unmap`, `CloneVmo`) is preserved as root-VMAR compatibility behavior, but normal ELF process construction uses delegated root/sub-VMAR capabilities.

## What Problem VMARs Solve

In legacy Unix/POSIX kernels, memory allocation relies on ambient calls like `mmap(NULL, size, ...)` or `brk()`. This leads to major problems in modern multi-threaded runtimes (like Rust, WASM engines, and JIT compilers):

* **Address Space Clobbering:** One thread or dynamic library mapping memory at a fixed address can accidentally overwrite or fragment an address range another component was preparing to use.
* **Lack of Delegation & Sub-sandboxing:** A process cannot safely say, *"I want to run this WASM module or JIT runtime in a strictly bounded 4GB virtual address box without giving it access to my entire 64-bit address space."*
* **Unbounded Memory Fragmentation:** Allocating and deallocating disparate buffers creates virtual address space holes that are difficult to reclaim in long-running services.

## How VMARs Work: The Tree Model

Instead of a flat array of page table entries, a process's virtual memory is modeled as a **hierarchical tree of VMAR capabilities**:

```
[ Root VMAR ] (Entire user address space: 0x000000000000 - 0x7FFFFFFFFFFF)
      │
      ├── [ Sub-VMAR: WASM Linear Memory Sandbox ] (Reserved: 0x1000000000 - 0x1100000000)
      │     ├── Mapping: Read-Only Code VMO
      │     └── Mapping: 4GB Guarded WASM Heap VMO
      │
      ├── [ Sub-VMAR: Native Stack & Heap ]
      │     ├── Mapping: Thread 1 Stack VMO (with Guard Pages)
      │     └── Mapping: Thread 2 Stack VMO (with Guard Pages)
      │
      └── [ Sub-VMAR: D1 Driver MMIO / Shared Buffers ]
            └── Mapping: NVMe Controller Doorbell Register VMO

```

1. **Root VMAR:** Created automatically when the kernel control plane creates a new process VM space. Represents the entire valid user address range.
2. **Sub-VMARs:** A process can carve out sub-regions with specific constraints (e.g., maximum size, alignment, base address restrictions, permissions).
3. **Mappings:** Actual physical memory containers (**VMOs**) can only be mapped into a valid leaf of a VMAR.

## Why VMARs Are Critical for BexOS

### In-Process Sandboxing (WASM / WebGPU Runtimes)

A native WASM runtime (like Wasmtime) relies heavily on reserving a contiguous **4GB virtual memory window with surrounding guard pages** to guarantee that WASM `i32` memory offsets can *never* read out of bounds without triggering a hardware fault.

* With VMARs, the host process mints a restricted `Sub-VMAR` and hands that capability handle directly to the WASM runner.
* The WASM runner can map/unmap memory within its private 4GB slice, but the kernel guarantees it cannot touch or map anything outside that sub-region.

### Fine-Grained Capability Delegation

In a microkernel, memory rights must obey the Principle of Least Privilege:

* A parent process can give a child worker thread a capability to map memory **only within a 64MB sub-range**.
* Even if the worker thread is compromised, it cannot tamper with the parent's page tables or map arbitrary DMA buffers across the rest of the address space.

### Atomic Teardown & Deallocation

Destroying an isolated component, sandbox, or dynamic thread pool requires one `vmar.destroy()` call rather than iterating over individual mappings. The kernel recursively unmaps the subtree and clears its hardware page tables atomically.

## Minimal Kernel FIDL Interface for VMARs

```fidl
library bexos.kernel.vm;

using bexos.kernel;

type VmarFlags = strict bits : uint32 {
    CAN_MAP_READ      = 0x00000001;
    CAN_MAP_WRITE     = 0x00000002;
    CAN_MAP_EXECUTE   = 0x00000004;
    CAN_MAP_SPECIFIC  = 0x00000008; // Allow mapping at explicit fixed sub-offsets
    COMPACT           = 0x00000010; // Allocate allocations close together
};

protocol Vmar {
    /// Allocate a nested sub-region within this VMAR
    CreateSubVmar(struct {
        offset uint64,
        size uint64,
        flags VmarFlags
    }) -> (resource struct {
        status bexos.kernel.Status,
        sub_vmar handle:VMAR,
        base_address uint64
    });

    /// Map a physical memory container (VMO) into this region
    MapVmo(resource struct {
        vmo handle:VMO,
        vmo_offset uint64,
        vmar_offset uint64,
        length uint64,
        flags VmarFlags
    }) -> (struct {
        status bexos.kernel.Status,
        mapped_address uint64
    });

    /// Unmap a specific address range inside this VMAR
    Unmap(struct {
        address uint64,
        length uint64
    }) -> (struct { status bexos.kernel.Status });

    /// Atomically destroy this VMAR and all nested mappings/sub-VMARs
    Destroy() -> (struct { status bexos.kernel.Status });
};

```

## VMAR design tradeoff

* **Without VMARs:** Legacy POSIX `mmap` behavior complicates WASM memory isolation, dynamic linking, and guard-page protection, with a risk of address-clobbering bugs.
* **With VMARs:** Hierarchical memory management supports zero-cost sub-process sandboxing and capability-driven memory ownership across the OS.

*
**In the kernel, implement strictly monotonic tick/elapsed time. Move all wall-clock, UTC, calendar, and timezone logic into userspace.**

Handling real-world "human" time inside ring 0 is a classic architectural anti-pattern for microkernels. Real-world time is subject to leap seconds, NTP network slewing, timezone changes, Daylight Saving Time, and RTC battery drift—none of which belong in the trusted computing base (TCB).

## Clock responsibilities

```
┌─────────────────────────────────────────────────────────────┐
│ USESPACE: Time Service (`bexos.time.TimeKeeper`)            │
│ • Wall-clock / UTC timestamp calculation                    │
│ • NTP / NTS network synchronization & RTC clock reading     │
│ • Leap second handling, timezones, DST adjustments          │
│ • Exposes WallClock FIDL interface & updates shared vDSO    │
└──────────────────────────────┬──────────────────────────────┘
                               │ Reads monotonic base + offset
                               ▼
┌─────────────────────────────────────────────────────────────┐
│ KERNEL: Strictly Monotonic Hardware Clocks                  │
│ • Raw Monotonic Nanoseconds (since boot / unhalted CPU cycles│
│ • Hardware Deadline Timers (TSC-Deadline, ARM Generic Timer)│
│ • Thread sleep queues & EDF scheduling deadlines            │
│ • Zero leap seconds, zero backwards stepping, zero timezones │
└─────────────────────────────────────────────────────────────┘

```

## What Stays in the Kernel (Monotonic Time Only)

The microkernel only needs to care about one concept: **elapsed physical time (ticks/nanoseconds)**.

### Purpose

* Calculating thread timeslice expiration and real-time EDF deadlines.
* Sleeping threads (`sys_thread_sleep_until(monotonic_deadline)`).
* Lock timeouts (`futex_wait(deadline)`).
* High-resolution performance counters for profiling.

* **Invariant:** Time *must* always flow forward at a constant rate ($t_{n+1} \ge t_n$). It must never step backward, freeze, or adjust for daylight savings.

## What Moves to Userspace (`TimeKeeper` Service)

All complexity regarding the calendar and global synchronization lives in an unprivileged userspace daemon (`time_service`):

* **RTC & Network Sync:** Reads the hardware Real-Time Clock (RTC) chip via standard D1 MMIO drivers on boot, then periodically polls NTP/NTS servers over the network.
* **Computing Wall Clock:** The service computes a simple translation formula:

$$\text{UTC Time} = \text{Kernel Monotonic Time} + \text{Offset}_{\text{UTC}}$$

* **Smooth Slewing:** When NTP detects clock drift, it gradually adjusts the $\text{Offset}$ factor (slewing) rather than suddenly jumping time backward, preventing timestamp glitches in user databases.
* **Timezones & Formatting:** Handled entirely by client WASM application libraries (e.g., standard Rust `chrono` / `time` crates) using local timezone databases.

## High Performance: Zero-Syscall Time via vDSO

Calling a kernel syscall just to read the current time (`clock_gettime`) adds IPC and context-switch overhead to benchmarks and logging loops.

Instead, use a **vDSO (virtual Dynamic Shared Object)** / Shared Read-Only VMO:

1. The kernel maps a single physical page read-only into every process's root VMAR.
2. This page contains:
* CPU timer frequency scaling multipliers ($TSC \to \text{nanoseconds}$).
* The active UTC offset managed by `time_service`.

3. User applications read the raw CPU counter instruction directly from hardware (`RDTSC` on x86, `CNTVCT_EL0` on ARM64) and compute the time entirely in user space in **$\approx$ 5–10 nanoseconds without a kernel trap**.

## Minimal Kernel FIDL Interface for Clocks

```fidl
library bexos.kernel.time;

using bexos.kernel;

type ClockType = strict enum : uint8 {
    MONOTONIC = 1;     // Elapsed ns since boot (excludes deep sleep)
    BOOT_TIME = 2;     // Elapsed ns including low-power sleep states
};

protocol Clock {
    /// Read raw monotonic nanoseconds (fallback if vDSO is unavailable)
    GetTime(struct { clock_type ClockType }) -> (struct {
        status bexos.kernel.Status,
        nanos uint64
    });

    /// Read the read-only shared vDSO memory page handle
    GetVdsoTimePage() -> (resource struct {
        status bexos.kernel.Status,
        vmo handle:VMO
    });
};

```

This keeps the microkernel tiny, deterministic, and isolated from network/timezone complexity while delivering sub-microsecond timer precision for real-time scheduling.
