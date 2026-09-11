# RFC 0007: Dependency-ordered service startup

- Created: 2026-08-26T18:18:51-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

Appd starts services and drivers in dependency waves, waiting for readiness before advancing. The design also considers dynamic dependency graphs and lazy activation.

## Design overview

`appd` should start services and drivers in dependency waves.

Starting all drivers and services simultaneously in parallel causes race conditions, IPC channel timeouts, and resource contention. Startup can use **discrete waves** or a **dynamic Directed Acyclic Graph (DAG)**.

## Rationale for phased startup

* **Hardware & Bus Dependencies (Drivers):** A storage controller (NVMe) cannot initialize until the PCIe bus driver and IOMMU are active; a filesystem service cannot mount partitions until the storage driver registers its block device.
* **Core IPC & Capability Infrastructure:** Higher-level services (like network stacks or UI compositors) need core broker services (`appd`, loggers, memory allocators, and identity vaults) running before they can bind FIDL endpoints.
* **Boot Contention:** Spawning dozens of processes at once spikes CPU scheduling queues and memory allocation, slowing down the critical path to a usable shell/desktop.

## Recommended four-wave boot pipeline

In a capability microkernel like BexOS, early startup naturally breaks into four distinct waves:

```
[ Bootloader / Kernel Trap ]
           │
           ▼
┌────────────────────────────────────────────────────────┐
│ Wave 0: Root Drivers & Bus Enumeration                 │
│ • Root bus driver (PCIe / ACPI / DeviceTree)           │
│ • Interrupt controller (GIC / APIC) & Timer            │
│ • Early debug UART / Console service                   │
└────────────────────────────────────────────────────────┘
           │ (Hardware buses ready)
           ▼
┌────────────────────────────────────────────────────────┐
│ Wave 1: Core System & Storage Providers (D1)           │
│ • Block drivers (NVMe / VirtIO-Blk / eMMC)             │
│ • Memory / VMO allocator services                      │
└────────────────────────────────────────────────────────┘
           │ (Storage & manifests accessible)
           ▼
┌────────────────────────────────────────────────────────┐
│ Wave 2: Filesystem and Platform Storage                │
│ • BexFS (consumes the wave-1 BlockDevice endpoint)     │
│ • BlobFS for /pkg                                      │
└────────────────────────────────────────────────────────┘
           │ (Namespaces available)
           ▼
┌────────────────────────────────────────────────────────┐
│ Wave 3: Platform Infrastructure Services               │
│ • appd / Package manager                        │
│ • Network stack & NIC drivers                          │
│ • Identity & Session services (user_service)           │
│ • GPU / Display controller drivers                     │
└────────────────────────────────────────────────────────┘
           │ (System UI & network ready)
           ▼
┌────────────────────────────────────────────────────────┐
│ Wave 4: User Environment & Applications                │
│ • BexOS Compositor / Window Manager                    │
│ • Shell / Terminal (WASI CLI)                          │
│ • Autostart User Apps & Background Daemons             │
└────────────────────────────────────────────────────────┘

```

## Discrete Waves vs. Dynamic Dependency Graph (DAG)

| Feature | Static Wave Levels (`wave = 0, 1, 2...`) | Dynamic DAG (systemd / Fuchsia style) |
| --- | --- | --- |
| **How it Works** | Barrier sync: Wave $N+1$ only starts after all services in Wave $N$ signal `READY`. | Services declare dependencies (`needs: ["bexos.fs.BlobFS"]`); services launch concurrently as requirements resolve. |
| **Complexity** | Very low (simple array iteration and readiness barriers in `appd`). | Higher (requires topological sort and dynamic state machines). |
| **Boot Speed** | Slightly slower (idle time waiting on the slowest component in a wave). | Maximum parallelism (starts services the microsecond their dependencies bind). |

## Implementation sequence

1. **Phase 1 (Bringup):** Use simple **static waves (Tiers 0–3)** in service definitions to get a deterministic, predictable boot sequence without race conditions.
2. **Readiness Signaling:** Never advance a wave on process spawn alone. Require services to send a FIDL `Ready()` signal or assert a `SIGNALED` bit on their control channel once their initialization is complete.
3. **Phase 2 (Optimization):** Transition to **lazy / on-demand startup** where `appd` passes unfulfilled FIDL channel endpoints to dependent services. The dependent service can boot immediately, and messages queue up until the provider service finishes initializing.

## V1 Manifest Contract

The bringup implementation uses an optional `Process.wave` field in app
manifests:

```proto
processes {
  name: "root_bus"
  runner: "elf"
  service: true
  wave: 0
}
```

- `wave: 0..N` marks the process for automatic appd startup in that static
  barrier wave.
- Omitting `wave` leaves the process manual-only. Appd must not include it in
  automatic boot; the user or a future management API can start it explicitly.
- `wave: 0` is a real automatic wave and is distinct from omitted/null.
- Appd starts every process in the current wave, waits for each launched
  process to report ready, and only then advances to the next numeric wave.
