# RFC-0072: Pluggable Component Runner Architecture and Execution Decoupling

* **Author:** BexOS Frameworks, Systems Architecture & Security Working Group
* **Status:** Implemented (see [current implementation](CURRENT.md))
* **Target Subsystems:** `appd`, `idl/bexos/component/runner.fidl`, `native_runner`, `wasm_runner`, `starnix_runner`, `lib/userspace`
* **Applicability:** Native Apps, WebAssembly Applications, Linux/Starnix Containers, System Daemons

---

## 1. Summary

This RFC specifies the formal decoupling of application orchestration from execution environments within BexOS. It introduces:

1. **Runner Decoupling from `appd`:** Stripping all runtime interpretation, JIT compilation, and container execution logic from the core application manager (`appd`).
2. **The `bexos.component.runner` Protocol Suite:** A standardized, bidirectional FIDL contract (`ComponentRunner`, `ComponentController`) governing the instantiation, control, and lifecycle termination of components.
3. **Isolated D2 Execution Runners:** Implementing runtime engines (`wasm_runner`, `starnix_runner`) as unprivileged, sandboxed D2 services communicating with `appd` strictly via capability routing.
4. **The Fixed BootFS Bootstrap Host:** A statically linked `native_runner` is the only execution host loaded from BootFS. It maps either a native target or a registered runner provider into the process that `appd` pre-created inside the component job.

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
* Parses application prototxt manifests.
* Validates and resolves capability routes (`/svc/bexos.net.SocketProvider`).
* Creates an isolated one-process `zx.Handle:JOB` constrained by immutable
  package, resource, hardware, realtime, and process-count policy.
* Delivers the attenuated job, package/dependency directories, opaque program
  metadata, and versioned `Startup` resources to the designated runner.


* **Runners (Execution Engines):**
* Implement `bexos.component.runner.ComponentRunner`.
* Ingest the binary payload (ELF, WASM, Linux binary) from the resolved package-directory capability. The private native-host bootstrap path may use bounded immutable VMOs before directory services exist.
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
│ • Runs inside component job/process│ │ • Enters D0 Restricted Mode         │
│ • Serves `ComponentController`      │ │ • Serves `ComponentController`      │
└─────────────────────────────────────┘ └─────────────────────────────────────┘

```

---

## 4. The Bootstrap Boundary: The Fixed BootFS Native Host

Decoupling runners presents a bootstrap dependency problem: *if all runners are external components, what launches the runner components themselves?*

BexOS resolves this with one fixed, statically linked `native_runner` in
BootFS. Appd's bootstrap loader is restricted to that one verified image and
rejects dynamic dependencies. For each component appd pre-creates the immutable
job and initial process, then gives the disposable native host only attenuated
mapping, start, transfer, management, and inspection capabilities. The host
maps either the native target or the platform-registry-selected WASM/Starnix
provider. It cannot create another process or change job policy.

WASM and Starnix providers are independently signed system-image packages, not
BootFS executables or appd compile-time data. A provider receives the standard
`ComponentRunner` server endpoint in the component process. For direct native
ELF, the native host itself serves the same standard protocol.



---

## 5. Interface Definition Language (FIDL) Specification

The core protocol definitions reside in `idl/bexos/component/runner.fidl`.
The normative source contains the complete bounded resource structs; the
protocol shape is:

```fidl
library bexos.component.runner;

using bexos.kernel;

struct ProgramMetadata {
    type_url string:256;
    payload vector<uint8>:65536;
};

protocol ComponentRunner {
    1: Start(resource struct {
        start_info ComponentStartInfo;
        controller server_end:ComponentController;
        events client_end:ComponentRunnerEvents;
    });
};

protocol ComponentController {
    1: Stop();
    2: Kill();
    3: SendSignal(struct { signal uint32; });
    4: Connect(resource struct { connection ServiceConnection; });
};

protocol ComponentRunnerEvents {
    1: OnReady(struct { status bexos.kernel.Status; });
    2: OnStop(struct {
        status bexos.kernel.Status;
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
2. It decodes `start_info.program` and loads the `.wasm` bytecode from
   `package_dir`.
3. Cranelift compiles admitted raw WASM to Pulley bytecode, or the runner uses
   an exactly matched trusted precompiled artifact; it does not execute native
   JIT code.
4. It configures WASI from the namespace directories in the versioned
   `Startup` resource.
5. Starts execution in the pre-created component process and reports readiness.
6. Returns control; lifecycle messages route across the `ComponentController` channel.



### 6.2 Starnix Runner (`starnix_runner`)

* **Role:** Executes unmodified Linux binaries and OCI container workloads (RFC-0050).
* **Execution Flow:**
1. `starnix_runner` receives `ComponentStartInfo`.
2. Resolves Linux ELF binaries, libraries, and mount parameters from `package_dir`.
3. Allocates thread register state buffers and invokes `zx_restricted_bind_state()`.
4. Launches the Linux entrypoint in CPU Ring 3 / EL0 under Restricted Mode.
5. Intercepts hardware `syscall` traps, emulating Linux kernel operations in userspace before re-entering restricted execution.



### 6.3 Dioxus WASM profile

* **Role:** Runs Dioxus component-model applications inside each application's isolated `wasm_runner` process; there is no separate Dioxus runner kind.
* **Execution Flow:**
1. Sets up the application event loop, binding `bexos.ui.ViewProvider` from
   the versioned `Startup` namespace resources.
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
  │ 5. ComponentRunnerEvents.OnStop(status: OK, exit_code: 0)│
  │◄─────────────────────────────────────────────────────────┤
  │                                                          │
  │ 6. `appd` closes `app_job` and reclaims resources        │
  ▼                                                          ▼

```

* **Graceful Termination:** `appd` calls `Stop()`. The runner invokes runtime cancellation hooks (e.g., triggering `SIGTERM` in Starnix, or setting an exit flag in a WASM event loop).
* **Escalation / Timeout:** If the instance fails to emit `OnStop` within five seconds, `appd` terminates the retained component job. A controller `Kill()` request has the same immediate job-level outcome.

---

## 8. Security & Sandbox Boundary

1. **Runner Zero-Ambience:** Runners are unprivileged D2 components. They cannot invent capabilities or inspect applications outside of the `ComponentStartInfo` resources explicitly delegated to them by `appd`.
2. **Attenuated Job Hierarchies:** The application's process is created inside a child job allocated by `appd`, not by the runner. The runner cannot elevate the application's CPU or memory allowances beyond what `appd` specified in the job policy.
3. **Namespace Isolation:** Applications have no ambient access to the root filesystem or global service namespaces. Runtimes can only bind interfaces exposed through the versioned `Startup` resources carried by `ComponentStartInfo`.
4. **Crash Containment:** If a bug in `wasm_runner` causes a fatal segfault or panic:
* Only that component job terminates; for WASM the application and its runner
  intentionally share that isolated process.
* `appd` receives a `ZX_CHANNEL_PEER_CLOSED` on the `ComponentController` interface.
* `appd` logs the crash, alerts telemetry, and respawns the runner on demand without interrupting the rest of the OS.



---

## 9. Implementation Status

### Phase 1: Protocol Standardization & In-Tree Separation

* Complete: `bexos.component.runner` is defined in
  `idl/bexos/component/runner.fidl`.
* Complete: the full ELF mapper lives in `lib/native_loader`; appd retains only
  the static loader for the fixed BootFS `native_runner` image.
* Complete: appd pre-creates the immutable component job and process and
  delegates only the rights required to map and start it.

### Phase 2: Standalone `wasm_runner`

* Complete: `services/wasm_runner` is an independently packaged, signed
  provider selected by the platform registry.
* Complete: it serves `ComponentRunner` directly and preserves the WASI
  Preview 2, component-model, migration, limits, and Dioxus profiles.
* Complete: host tests cover provider validation and sandboxed WASM launch
  behavior.

### Phase 3: Integration with Starnix & UI Runtimes

* Complete: `starnix_runner` serves `ComponentRunner`; signal forwarding uses
  `ComponentController.SendSignal` instead of a private control transport.
* Complete: application prototxt manifests retain their runner strings, while
  provider identity and executable selection come from platform prototxt.
* Complete: the combined native/WASM/Dioxus/Starnix isolation and provider
  replacement scenario builds for both architectures. Live guest execution is
  recorded separately in [the current implementation](CURRENT.md).
