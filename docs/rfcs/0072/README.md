# RFC-0072: Pluggable Component Runner Architecture and Execution Decoupling

* **Author:** BexOS Frameworks, Systems Architecture & Security Working Group
* **Status:** Proposed
* **Target Subsystems:** `appd`, `sdk/fidl/bexos.component.runner`, `wasm_runner`, `starnix_runner`, `libbexos_runtime`
* **Applicability:** Native Apps, WebAssembly Applications, Linux/Starnix Containers, System Daemons

---

## 1. Summary

This RFC specifies the formal decoupling of application orchestration from execution environments within BexOS. It introduces:

1. **Runner Decoupling from `appd`:** Stripping all runtime interpretation, JIT compilation, and container execution logic from the core application manager (`appd`).
2. **The `bexos.component.runner` Protocol Suite:** A standardized, bidirectional FIDL contract (`ComponentRunner`, `ComponentController`) governing the instantiation, control, and lifecycle termination of components.
3. **Isolated D2 Execution Runners:** Implementing runtime engines (`wasm_runner`, `starnix_runner`) as unprivileged, sandboxed D2 services communicating with `appd` strictly via capability routing.
4. **The Minimal In-Tree Bootstrap Runner:** Retaining a strictly minimal, non-JIT static ELF loader inside `appd` exclusively to break circular startup dependencies and bootstrap early root system services.

---

## 2. Motivation

In earlier designs, `appd` served as both the component coordinator (topology graph, manifest parsing, capability routing) and the execution engine. Housing execution engines—specifically complex runtimes like Wasmtime, Dioxus bytecode interpreters, or the Starnix Linux translation layer—inside the central application manager breaks key platform invariants:

* **Failure Domain Collapse:** A memory corruption vulnerability, unhandled panic, or JIT miscompilation in an application runtime crashes `appd`. Because `appd` holds the root application topology and handles lifecycle state, a crash halts all running applications across the system.
* **Privilege Creep & Security Boundary Violations:** `appd` maintains privileged microkernel handles, including child job creation rights and capability routing authority. Loading untrusted or complex runtime execution stacks into the `appd` address space violates the principle of least privilege.
* **Lack of Independent Updates:** Updating or rolling back a WebAssembly compiler or Linux emulation engine requires restarting `appd`, tearing down active user sessions and system utilities.
* **In-Process Dynamic Libraries (`dlopen` / `.so`) Are Insufficient:** Loading runners as in-process plugins fails to provide hardware-backed memory isolation, process address space separation, or fault containment.

Decoupling runners into standalone, sandboxed D2 components ensures that runtimes crash in isolation, can be hot-reloaded, and operate under attenuated microkernel capabilities.

---

## 3. Architecture & System Topology

The platform separates **coordination** from **execution**:

* **`appd` (Coordinator / Topology Manager):**
* Parses manifests (`app.cml`).
* Validates and resolves capability routes (`/svc/bexos.net.SocketProvider`).
* Creates an empty, isolated child `zx.Handle:JOB` constrained by declared CPU, memory, and priority policies.
* Delivers the job handle, package directory, and namespace map to the designated runner.


* **Runners (Execution Engines):**
* Implement `bexos.component.runner.ComponentRunner`.
* Ingest the binary payload (ELF, WASM, Linux binary) from the resolved package channel or VMO.
* Initialize the execution context (construct address space, map vDSO, compile bytecode, or enter microkernel Restricted Mode).
* Control instance execution via `bexos.component.runner.ComponentController`.



```
┌─────────────────────────────────────────────────────────────────────────────┐
│ `appd` (D1 Application Coordinator / Capability Router)                     │
│                                                                             │
│  1. Ingests manifest: `program: { runner: "wasm", binary: "bin/app.wasm" }` │
│  2. Resolves incoming capabilities: `/svc/bexos.ui.ViewProvider`            │
│  3. Creates empty child sandbox: `app_job = job.create_child_job(...)`      │
│  4. Locates runner capability provider for "wasm"                           │
└──────────┬───────────────────────────────────────┬──────────────────────────┘
           │ FIDL: `ComponentRunner.Start(...)`    │ FIDL: `ComponentRunner.Start(...)`
           ▼                                       ▼
┌─────────────────────────────────────┐ ┌─────────────────────────────────────┐
│ `wasm_runner` (D2 Sandboxed Service)│ │ `starnix_runner` (D2 Sandboxed)     │
│ • Links `wasmtime` engine           │ │ • Pure-Rust Linux translation       │
│ • Spawns thread inside `app_job`    │ │ • Enters D0 Restricted Mode         │
│ • Serves `ComponentController`      │ │ • Serves `ComponentController`      │
└─────────────────────────────────────┘ └─────────────────────────────────────┘

```

---

## 4. The Bootstrap Boundary: The Minimal In-Tree ELF Loader

Decoupling runners presents a bootstrap dependency problem: *if all runners are external components, what launches the runner components themselves?*

BexOS resolves this by splitting ELF execution into two tiers:

1. **The Internal Bootstrap ELF Loader (`appd::builtin_elf`):**
* Statically compiled into `appd`.
* Stripped of dynamic library linking, profiling hooks, or complex debugging hooks.
* **Scope-Restricted:** Hardcoded to launch only platform root services originating from the verified, read-only BootFS image (`/boot/bin/*`), including:
* `driver_manager`
* `pkgd`
* `networkd`
* `wasm_runner`
* `starnix_runner`




2. **External Application Runners:**
* All user-facing native apps, third-party services, WASM packages, and container runtimes execute via external runners.



---

## 5. Interface Definition Language (FIDL) Specification

The core protocol definitions reside in `idl/bexos/component/runner/runner.fidl`:

```fidl
library bexos.component.runner;

using bexos.kernel;

/// Represents a single mapped entry in a component's incoming namespace
type ComponentNamespaceEntry = resource struct {
    /// Target path within the component's virtual root (e.g., "/svc", "/data")
    path string:64;

    /// Channel handle to the directory serving capabilities for this path
    directory zx.Handle:CHANNEL;
};

/// Parameters provided by appd to instantiate a component
type ComponentStartInfo = resource struct {
    /// Canonical resolved URL of the component
    resolved_url string:256;

    /// Directory channel exposing the component's package contents (/pkg)
    package_dir zx.Handle:CHANNEL;

    /// Fully assembled namespace dictionary routed to the component
    namespace vector<ComponentNamespaceEntry>:32;

    /// Freeform program metadata block extracted from manifest
    program_metadata string:4096;

    /// Attenuated child job created by appd to house the running process
    job zx.Handle:JOB;

    /// Channel where the component serves its outgoing capabilities (/svc)
    outgoing_dir zx.Handle:CHANNEL;
};

@discoverable
protocol ComponentRunner {
    /// Instructs the runner to instantiate and begin executing a component
    Start(resource struct {
        start_info ComponentStartInfo;
        controller server_end:ComponentController;
    });
};

protocol ComponentController {
    /// Requests a clean, graceful shutdown of the component instance
    Stop();

    /// Immediately and forcefully terminates the component's job tree
    Kill();

    /// Event emitted by the runner when the component terminates
    -> OnStop(struct {
        termination_status bexos.kernel.Status;
        exit_code int64;
    });
};

```

---

## 6. Runner Execution Models

### 6.1 WebAssembly Runner (`wasm_runner`)

* **Role:** Executes modular, sandboxed WASM applications (e.g., background workers, utilities, and WASI modules).
* **Execution Flow:**
1. `wasm_runner` receives `ComponentStartInfo`.
2. It reads `binary` from `start_info.program_metadata` and loads the `.wasm` bytecode from `package_dir`.
3. Pre-compiles the module using an in-process Cranelift JIT or executes pre-compiled bytecode.
4. Configures WASI context: maps `start_info.namespace` directory channels to synthetic WASI file descriptors.
5. Spawns an initial thread directly within `start_info.job`.
6. Returns control; lifecycle messages route across the `ComponentController` channel.



### 6.2 Starnix Runner (`starnix_runner`)

* **Role:** Executes unmodified Linux binaries and OCI container workloads (RFC-0050).
* **Execution Flow:**
1. `starnix_runner` receives `ComponentStartInfo`.
2. Resolves Linux ELF binaries, libraries, and mount parameters from `package_dir`.
3. Allocates thread register state buffers and invokes `zx_restricted_bind_state()`.
4. Launches the Linux entrypoint in CPU Ring 3 / EL0 under Restricted Mode.
5. Intercepts hardware `syscall` traps, emulating Linux kernel operations in userspace before re-entering restricted execution.



### 6.3 Dioxus Native Runner (`dioxus_runner`)

* **Role:** Manages high-performance GUI applications using Stylo, Taffy, and Dioxus signals.
* **Execution Flow:**
1. Sets up the application event loop, binding `bexos.ui.ViewProvider` from `start_info.namespace`.
2. Maps shared font, locale, and timezone VMOs into the application’s address space.
3. Spawns the Dioxus reactive runtime thread inside the designated child job.



---

## 7. Component Lifecycle & Teardown Handshake

Graceful and forceful shutdowns follow an asynchronous handshake:

```
`appd`                                                 `wasm_runner`
  │                                                          │
  │ 1. ComponentController.Stop()                            │
  ├─────────────────────────────────────────────────────────►│
  │                                                          │ 2. Issues runtime shutdown
  │                                                          │    signal to thread
  │                                                          │
  │                                                          │ 3. Thread flushes data
  │                                                          │    and exits
  │                                                          │
  │                                                          │ 4. `wasm_runner` reaps
  │                                                          │    child thread
  │                                                          │
  │ 5. ComponentController.OnStop(status: OK, exit_code: 0)  │
  │◄─────────────────────────────────────────────────────────┤
  │                                                          │
  │ 6. `appd` closes `app_job` and reclaims resources        │
  ▼                                                          ▼

```

* **Graceful Termination:** `appd` calls `Stop()`. The runner invokes runtime cancellation hooks (e.g., triggering `SIGTERM` in Starnix, or setting an exit flag in a WASM event loop).
* **Escalation / Timeout:** If the instance fails to emit `OnStop` within a defined deadline (e.g., 5 seconds), `appd` invokes `Kill()`, immediately tearing down the underlying `zx.Handle:JOB` at the microkernel level.

---

## 8. Security & Sandbox Boundary

1. **Runner Zero-Ambience:** Runners are unprivileged D2 components. They cannot invent capabilities or inspect applications outside of the `ComponentStartInfo` payloads explicitly delegated to them by `appd`.
2. **Attenuated Job Hierarchies:** The application's process is created inside a child job allocated by `appd`, not by the runner. The runner cannot elevate the application's CPU or memory allowances beyond what `appd` specified in the job policy.
3. **Namespace Isolation:** Applications have no ambient access to the root filesystem or global service namespaces. Runtimes can only bind interfaces exposed within `start_info.namespace`.
4. **Crash Containment:** If a bug in `wasm_runner` causes a fatal segfault or panic:
* Only the runner process and its associated applications terminate.
* `appd` receives a `ZX_CHANNEL_PEER_CLOSED` on the `ComponentController` interface.
* `appd` logs the crash, alerts telemetry, and respawns the runner on demand without interrupting the rest of the OS.



---

## 9. Implementation Roadmap

### Phase 1: Protocol Standardization & In-Tree Separation

* Finalize `bexos.component.runner` FIDL definitions in `sdk/fidl/bexos.component.runner/`.
* Strip non-ELF execution code from `appd`.
* Restrict `appd::builtin_elf` to launch exclusively signed binaries residing in `/boot/bin`.

### Phase 2: Standalone `wasm_runner`

* Implement `services/wasm_runner` as an independent Rust daemon.
* Wire the `ComponentRunner` FIDL protocol to the Wasmtime engine.
* Verify launching a sandboxed WASI CLI tool through `appd` capability handoffs.

### Phase 3: Integration with Starnix & UI Runtimes

* Connect `starnix_runner` to expose the `ComponentRunner` protocol.
* Implement declarative runner routing in application manifests (`app.cml`).
* Verify concurrent execution of native ELF system daemons, WASM applications, and Starnix Linux containers managed through independent runner processes.