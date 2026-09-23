# RFC-0070: Linux Binary Emulation Runtime (Starnix Port), Kernel Restricted Execution Mode, and OCI Container Hosting

* **Author:** BexOS Systems Architecture & Compatibility Working Group
* **Status:** Proposed
* **Target Subsystems:** `kernel` (D0), `sdk/fidl/bexos.kernel`, `starnix_runner`, `pkgd`, `netstack`, `bexfs`, `vswitchd`
* **Applicability:** Unmodified Linux Binaries, Android Runtimes, OCI Containers (Docker/Podman/Kubernetes workloads)

---

## 1. Summary

This RFC specifies the architecture for executing unmodified Linux application binaries and standard OCI Linux container bundles on BexOS without running virtual machines or native Linux kernels. It defines:

1. **D0 Microkernel Restricted Execution Mode:** Syscall-transport primitives (`zx_restricted_bind_state`, `zx_restricted_enter`, `zx_restricted_kick`) allowing threads to execute arbitrary Ring 3 / EL0 code while trapping hardware `syscall`/`svc` instructions back into an unprivileged userspace translation runner.
2. **Porting Strategy for Fuchsia Starnix:** Forking and retargeting the pure-Rust Starnix subsystem into the BexOS Bazel monorepo, backed by a microkernel compatibility shim (`libs/compat/zircon`).
3. **Subsystem Resource Mapping:** Translating Linux filesystem semantics, task trees, signals, memory management (`mmap`, `brk`), and synchronization primitives onto BexOS microkernel capabilities (`VMO`, `VMAR`, Futexes).
4. **OCI Container Lifecycle Pipeline:** Integrating `pkgd` to pull and unpack OCI rootfs layers into persistent `bexfs` storage, mapping Linux namespaces/cgroups onto microkernel Job hierarchies, and providing network connectivity via `bexos.net.SocketProvider` and `vswitchd`.

---

## 2. Motivation

Modern platforms require vast application ecosystems that cannot immediately be rewritten for native microkernel APIs. While BexOS provides a memory-safe, capability-driven environment for native Rust/Dioxus components, critical workloads (database engines, developer tooling, web servers, AI runtimes, Android runtimes) assume the standard Linux Application Binary Interface (ABI).

Traditional approaches exhibit major drawbacks:

* **Hardware Hypervisors / MicroVMs (e.g., Firecracker, QEMU):** Running a complete Linux guest kernel incurs duplicated memory footprints (30–100 MB per VM idle baseline), slow cold-boot latencies, and indirect host storage/network virtualization bottlenecks.
* **Clean-Room Compatibility Emulators:** Emulating the 450+ Linux syscalls from scratch requires years of engineering and struggles with obscure edge cases (POSIX signal masking, `io_uring`, `openat2`, `clone3`, pseudo-filesystems like `/proc` and `/sys`).
* **In-Kernel Emulation (WSL 1 style):** Putting a Linux personality into Ring 0 introduces immense attack surface into the trusted computing base, violating the microkernel's least-privilege principles.

By porting Fuchsia's memory-safe Rust **Starnix** runtime into userspace and backing it with hardware-assisted CPU trap reflection, BexOS runs Linux binaries at native execution speed with microkernel process isolation.

---

## 3. Overall Architecture

The Linux binary runs as an unprivileged restricted thread within the Starnix runner process hierarchy. System calls do not hit Ring 0 Linux code; instead, the microkernel suspends the thread and vector-jumps to the Starnix translation loop in normal userspace mode.

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ RESTRICTED USERSPACE DOMAIN (Ring 3 / EL0)                                  │
│ Unmodified Linux Container Process (e.g., Alpine / Debian / Python / Redis) │
│ Issues: `syscall` (x86_64) or `svc #0` (AArch64)                            │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ Hardware Trap
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ D0 MICROKERNEL                                                              │
│ 1. Intercepts architectural trap from restricted execution context          │
│ 2. Saves GPR state frame into thread's pre-bound RestrictedState VMO        │
│ 3. Swaps CPU context back to Normal Mode (Starnix Host Runner Stack)        │
│ 4. Vector-jumps directly to userspace entrypoint: `vector_table_ptr`        │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ Direct Normal-Mode Jump
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ `starnix_runner` (D2 Sandboxed Process in Rust)                             │
│                                                                             │
│  [ Trap Dispatch Loop ]                                                     │
│  • Reads register frame (`rax` = syscall nr, `rdi`, `rsi`, `rdx`, ...)      │
│  • Emulates Linux semantics (sys_read, sys_epoll_ctl, sys_clone3)           │
│                                                                             │
│  [ Subsystem Backends ]                                                     │
│  ├── VFS Layer: Mapped to local `bexfs` or overlay VMO filesystems          │
│  ├── Memory Engine: Manipulates user address space via `zx_vmar_*`          │
│  ├── Sockets: Routes `AF_INET`/`AF_INET6` over `bexos.net.SocketProvider`   │
│  └── Task/Process: Subdivided within delegated caller `zx.Handle:JOB`       │
│                                                                             │
│  • Writes return code into register frame (`rax = result`)                  │
│  • Re-enters restricted mode: `zx_restricted_enter(0, vector, context)`     │
└─────────────────────────────────────────────────────────────────────────────┘

```

---

## 4. D0 Microkernel: Syscall-Transport Restricted Mode

To support Starnix without ambient kernel privileges, the microkernel implements a dedicated syscall transport protocol.

### 4.1 Kernel FIDL Specification (`//sdk/fidl/bexos.kernel/restricted.fidl`)

```fidl
library bexos.kernel;

type RestrictedReason : uint32 {
    SYSCALL = 1;
    EXCEPTION = 2;
    KICK = 3;
};

type ArchitectureRegisters = flexible union {
    1: x86_64 struct {
        rdi uint64;
        rsi uint64;
        rdx uint64;
        rcx uint64;
        r8 uint64;
        r9 uint64;
        rax uint64;
        rbx uint64;
        rbp uint64;
        r10 uint64;
        r11 uint64;
        r12 uint64;
        r13 uint64;
        r14 uint64;
        r15 uint64;
        rip uint64;
        rsp uint64;
        rflags uint64;
        fs_base uint64;
        gs_base uint64;
    };
    2: arm64 struct {
        r array<uint64, 31>; // x0 - x30
        sp uint64;
        pc uint64;
        cpsr uint64;
        tpidr_el0 uint64;
        tpidrro_el0 uint64;
    };
};

struct RestrictedState {
    registers ArchitectureRegisters;
};

@transport("Syscall")
protocol Restricted {
    /// Binds an anonymous VMO to hold the thread's trapped register state.
    /// Must be invoked before entering restricted mode.
    restricted_bind_state(resource struct {
        options uint32;
        state_vmo zx.Handle:VMO;
    }) -> (struct {
        status Status;
    });

    /// Unbinds the thread's restricted state frame.
    restricted_unbind_state(struct {
        options uint32;
    }) -> (struct {
        status Status;
    });

    /// Enters restricted execution.
    /// Does not return directly on success; exits to normal mode via `vector_table_ptr`.
    restricted_enter(struct {
        options uint32;
        vector_table_ptr uint64;
        context_ptr uint64;
    }) -> (struct {
        status Status;
    });

    /// Asynchronously kicks a thread out of restricted mode back to normal mode.
    restricted_kick(resource struct {
        thread zx.Handle:THREAD;
        options uint32;
    }) -> (struct {
        status Status;
    });
};

```

### 4.2 Hardware Context Transition Lifecycle

When `zx_restricted_enter()` is executed:

1. **Mode Transition:** The D0 microkernel marks the calling thread's scheduler state as `RESTRICTED`.
2. **Register Swap:** Normal mode registers (Starnix's RSP, RIP, callee-saved registers) are stashed in the thread's internal TCB (Thread Control Block). The CPU registers are loaded from the bound `RestrictedState` VMO.
3. **Execution (Ring 3 / EL0):** The CPU returns to userspace at the Linux program's `rip`/`pc`.
4. **Architectural Trap:**
* On x86-64: A `syscall` instruction vector-traps to the kernel MSR handler (`IA32_LSTAR`).
* On AArch64: An `svc #0` instruction triggers an `ESR_EL1` synchronous trap.


5. **State Capture:** The microkernel detects the thread is in `RESTRICTED` mode. It commits the current register state directly into the thread's `RestrictedState` VMO.
6. **Normal Mode Return:** The kernel restores the normal mode host registers and jumps to `vector_table_ptr`, passing `context_ptr` in register `rdi`/`x0` and `RestrictedReason::SYSCALL` in `rsi`/`x1`.

---

## 5. Starnix Subsystem Porting & BexOS Bridge

Rather than writing a clean-room emulator, BexOS imports Fuchsia's memory-safe `starnix/kernel` Rust crates under `//third_party/starnix/` using a compatibility adapter.

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ `starnix_core` / `starnix_kernel` (Imported Upstream Rust Crates)           │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ Calls `fuchsia_zircon`
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ `libs/compat/zircon` (BexOS Zircon Compatibility Facade)                    │
│                                                                             │
│  Maps Zircon Primitives ──► BexOS Microkernel Native System Calls           │
│  • `zx::vmo`            ──► `bexos_kernel::vmo`                             │
│  • `zx::vmar`           ──► `bexos_kernel::vmar`                            │
│  • `zx::channel`        ──► `bexos_kernel::channel`                         │
│  • `zx::job`            ──► `bexos_kernel::job`                             │
│  • `zx::restricted_*`   ──► Generated Syscall Stubs via `restricted.fidl`   │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ Standard vDSO Link
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ BexOS vDSO (`libbexos_vdso.so`)                                             │
└─────────────────────────────────────────────────────────────────────────────┘

```

### 5.1 Subsystem Translation Mapping

#### 1. Memory Management (`mmap`, `brk`, `mprotect`)

* Linux processes expect a unified linear address space.
* Starnix maps this using a dedicated restricted `zx.Handle:VMAR` (Virtual Memory Address Region) allocated during process creation.
* `mmap(MAP_ANONYMOUS)` allocates a `VMO` and maps it into the restricted VMAR via `zx_vmar_map()`.
* File-backed `mmap` calls into the Starnix VFS, obtaining a read/write VMO handle representing the underlying storage node and mapping it with corresponding `ZX_VM_PERM_*` flags.

#### 2. Virtual File System (VFS) & Mount Table

* Starnix maintains an internal, in-memory inode/dentry VFS graph in safe Rust.
* **Rootfs Mount:** Bound to a local `bexfs` storage volume or read-only unpacked VMO image.
* **Pseudo Filesystems:** Starnix generates `/proc`, `/sys`, and `/dev` dynamically in memory, emulating files like `/proc/cpuinfo`, `/proc/self/maps`, and `/dev/null`.
* **Inter-Container Volumes:** Directories are backed by isolated sub-paths of `/data/containers/<id>/rootfs`.

#### 3. Task Model & Threading (`clone`, `fork`, `pthread_create`)

* `fork()` / `clone(SIGCHLD)` creates a new BexOS child process within the container's parent `zx.Handle:JOB`. The memory space is duplicated using `zx_vmo_create_child(..., ZX_VMO_CHILD_SNAPSHOT)`.
* `clone(CLONE_VM | CLONE_THREAD)` spawns a new BexOS thread inside the existing process, assigning it a separate `RestrictedState` frame.

#### 4. Synchronization (`futex`, `futex2`)

* Linux futex addresses map directly to native BexOS microkernel futex primitives (`zx_futex_wait`, `zx_futex_wake`).
* Priority inheritance and robust futex lists are managed by Starnix's task tracker.

---

## 6. OCI Container Ingestion & Execution Pipeline

To run standard Linux container images (Docker / OCI format), the runtime orchestrator links `pkgd`, `bexfs`, and `starnix_runner`.

```
┌──────────┐               ┌────────────────┐               ┌──────────────────┐
│  `pkgd`  │               │ `containerd`   │               │ `starnix_runner` │
└────┬─────┘               └───────┬────────┘               └────────┬─────────┘
     │                             │                                 │
     │ 1. `bex container run`      │                                 │
     │    Pulls OCI Image Manifest │                                 │
     │    (alpine:latest)          │                                 │
     │◄────────────────────────────┤                                 │
     │                             │                                 │
     │ 2. Downloads & Verifies     │                                 │
     │    TUF / Content Blobs      │                                 │
     ├────────────────────────────►│                                 │
     │                             │                                 │
     │                             │ 3. Unpack into Storage Volume   │
     │                             │    `/data/containers/c1/rootfs` │
     │                             │                                 │
     │                             │ 4. Spawn Sandboxed Job          │
     │                             │    Inject Network Socket Channel│
     │                             │    `CreateContainer(c1_job, ...)`
     │                             ├────────────────────────────────►│
     │                             │                                 │ 5. Parse ELF Header
     │                             │                                 │    (`/bin/sh`)
     │                             │                                 │ 6. Bind Restricted
     │                             │                                 │    State & Enter

```

### 6.1 Container Configuration Specification (`container.json`)

`containerd` generates an OCI runtime configuration passed to Starnix over FIDL:

```json
{
  "container_id": "c1-alpine-redis",
  "rootfs": {
    "storage_vmo": "handle:0x2f",
    "readonly": false
  },
  "entrypoint": ["/usr/bin/redis-server", "--port", "6379"],
  "environment": [
    "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
    "TERM=xterm"
  ],
  "namespaces": {
    "pid": "isolated",
    "mount": "isolated",
    "network": "bridge"
  },
  "resources": {
    "cpu_shares": 1024,
    "memory_limit_bytes": 536870912
  }
}

```

### 6.2 Network Integration: Bridge vs. Isolated Stacks

Containers acquire network connectivity using two distinct topologies established in RFC-0048:

* **Mode 1: Direct Socket Bridge (Lightweight):**
Starnix translates POSIX socket calls (`socket(AF_INET, SOCK_STREAM, 0)`) directly into IPC messages sent across `bexos.net.SocketProvider`. The container shares the system default IP address with zero NAT or network device emulation overhead.
* **Mode 2: Virtual L2 Network Namespace (`vswitchd` + Virtual NIC):**
When a container demands raw packet sockets, isolated firewall tables (`iptables`/`nftables`), or a private IP address:
1. `vswitchd` instantiates an emulated virtio-net or tap packet descriptor ring.
2. Starnix exposes this ring inside the container as `eth0`.
3. The container runs its own internal network stack or binds directly to an isolated `netstack` instance.



---

## 7. Security, Isolation, and Sandboxing Analysis

Running arbitrary Linux binaries introduces potential security risks. BexOS enforces strict microkernel containment around the emulation environment:

| Threat Vector | System Defense & Mitigation |
| --- | --- |
| **Linux Kernel CVE Exploit in Unmodified Binary** | Irrelevant. There is no Linux kernel running in supervisor mode. Exploits targeting Linux ring-0 vulnerabilities (e.g., eBPF bugs, slab corruptions, dirty COW) fail because the underlying host is the BexOS microkernel. |
| **Starnix Translation Bug / Rust Panic** | Starnix runs in a sandboxed D2 userspace job. A crash in Starnix terminates only that specific container or runner instance; the host OS, compositors, and system daemons remain unaffected. |
| **Privileged Escape via `setuid` / Root Access** | Linux "root" ($UID=0$) inside Starnix is an emulated integer. It grants capabilities only over the container's virtual filesystem and internal thread table. It conveys **zero** capabilities to BexOS kernel objects or host FIDL endpoints. |
| **Direct Hardware Access via Devices (`/dev/mem`, raw PCI)** | Starnix exposes only synthetic device nodes. Hardware MMIO, physical interrupts, and IOMMU capabilities are never surfaced into the Linux container. |
| **Resource Exhaustion (Fork Bomb / Memory Leak)** | Every container executes inside an attenuated BexOS child `zx.Handle:JOB`. The microkernel enforces hard limits on maximum process counts, thread counts, CPU deadlines, and physical memory footprint. If the limit is exceeded, the microkernel OOM manager halts the container without host impact. |

---

## 8. Implementation Roadmap

### Phase 1: Microkernel Restricted Mode

* Implement `@transport("Syscall")` interface in `//sdk/fidl/bexos.kernel/restricted.fidl`.
* Implement `sys_restricted_bind_state`, `sys_restricted_enter`, and `sys_restricted_kick` in the D0 microkernel.
* Add x86-64 `IA32_LSTAR` and AArch64 `ESR_EL1` trap interceptors to swap contexts between Restricted and Normal thread execution.
* Build automated unit test running an isolated raw `syscall` assembly instruction in Ring 3 and trapping back to a test harness.

### Phase 2: Zircon Compatibility Layer & Starnix Ingestion

* Vendor Fuchsia Starnix core crates (`starnix_core`, `starnix_kernel`) into `//third_party/starnix/` under Bazel.
* Implement `libs/compat/zircon` mapping Zircon calls to BexOS vDSO primitives.
* Verify Starnix boot sequence in a headless userspace process running a static "Hello World" Linux ELF binary.

### Phase 3: Filesystem & Syscall Expansion

* Implement Linux VFS rootfs mounting backed by read-only VMO images and read/write `bexfs` directories.
* Hook terminal I/O to native console and `scened` text buffers.
* Validate core POSIX functionality against the Linux Test Project (LTP) test suite (focusing on filesystem, memory, and signals).

### Phase 4: OCI Runtime & Networking

* Connect `starnix_runner` to `pkgd` via `libpkg_client` to resolve and unpack multi-layer OCI images.
* Bridge Linux socket APIs to `bexos.net.SocketProvider`.
* Demonstrate running standard container images (e.g., Alpine Linux `sh`, Python HTTP server, Redis) on BexOS.