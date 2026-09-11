# RFC 0017: Host bridge and remote debugging

- Created: 2026-08-27T20:10:46-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

Bexctl and debugd provide typed, capability-gated host-to-device control. The implemented QEMU socket transport is retained alongside the longer-term debugging protocols, transports, and tooling.

## Design overview

A dedicated host bridge, **`bexctl`**, communicates with the device-side remote debug daemon, **`debugd`**. This provides structured debugging in place of a traditional Bash or UNIX root shell.

In a capability OS, arbitrary ambient shells (like root bash) subvert fine-grained capability tracking and encourage non-hermetic state mutation. A structured RPC interface supports typed automation, CI/CD integration, end-to-end testing, and capability-gated debugging.

## Implemented: QEMU Socket v1

The first runnable slice is implemented for the QEMU board:

* `//services/debugd:debugd_elf` is packaged into the QEMU BootFS as
  `bexos.driver.debugd` and starts automatically after PCI, PL011, NVMe, and
  BexFS readiness.
* The deployed service is a std-linked Tokio daemon. Its public backend traits
  and debug-wire frame handlers are async, while the QEMU v1 frame schema,
  public host client behavior, and MMIO-backed UART transport remain unchanged.
* `//idl:debug_service_proto` defines the public
  `bexos.debug.v1.DebugService` contract. QEMU v1 uses the proto messages over
  a small framed byte transport instead of full HTTP/2 ConnectRPC.
* `//idl:kernel_fidl_rust` includes `KernelDebugControl.ListProcesses`; the
  bare-metal syscall route exposes it as kernel protocol id `6`.
* `//idl:app_debug_fidl_rust` defines the appd lifecycle debug model,
  and `services/appd::debug` has a host-testable process registry.
* `//host/debug_client:debug_client` and `//tools/bexctl:bexctl` provide the
  host Rust client and CLI:

```sh
bazel run //tools/bexctl:bexctl -- --socket /path/to/debugd.sock health
bazel run //tools/bexctl:bexctl -- --socket /path/to/debugd.sock ps
bazel run //tools/bexctl:bexctl -- --socket /path/to/debugd.sock exec debugd.version
```

QEMU v1 frames are:

```text
magic "BXD1"
u16 version = 1
u16 flags
u32 request_id
u32 method_id
u32 payload_len
payload bytes
```

The debug e2e target uses QEMU's serial UNIX socket backend:

```sh
bazel test --cxxopt=-std=c++17 --host_cxxopt=-std=c++17 //tools/qemu:debugd_e2e_test
```

`ExecCommand` remains constrained to diagnostic commands such as `debugd.version`
and `kernel.ps`. Interactive terminals use separate typed shell methods, usersd
authentication, and appd's preferred provider resolution. See the
[CLI reference](../../cli.md) and [TTY design](../0033/README.md); no production shell
application is bundled yet.

## Long-term architecture (future work)

The remaining sections describe the planned network and automation interfaces.
They are not additional BXD1 commands. The implemented contract is the checked-in
protobuf descriptor and the current CLI reference linked above.

### System Architecture Overview

```
+-------------------------------------------------------------------------+
| HOST SYSTEM (Developer Machine / CI Test Runner)                        |
|                                                                         |
|  [ bexctl CLI ]   [ Rust / TypeScript Test Harness (`bexos-e2e`) ]      |
|         │                                │                              |
|         └────────────────┬───────────────┘                              |
|                          │ ConnectRPC (HTTP/2 + Protobuf)               |
|                          ▼                                              |
|            [ Transport: Vsock / USB CDC-NCM / TCP:9090 ]                |
+──────────────────────────┬──────────────────────────────────────────────+
                           │
                           ▼
+─────────────────────────────────────────────────────────────────────────+
| BEXOS TARGET SYSTEM                                                     |
|                                                                         |
|  [ debugd ] (Unprivileged Service with `DEBUG_API_ACCESS` capability)   |
|     │                                                                   |
|     │ Translates ConnectRPC ──► Kernel & System FIDL Protocols          |
|     │                                                                   |
|     ├── bexos.kernel.debug.DebugControl (Task inspection, core dumps)   |
|     ├── bexos.appd.debug.LifecycleDebug (Inject packages, mock bounds)  |
|     ├── bexos.log.LogStream             (Real-time structured logging)  |
|     ├── bexos.input.Inject              (Synthetic touch, key events)   |
|     └── bexos.storage.Snapshot          (State rollback for test trees) |
+-------------------------------------------------------------------------+

```

### The Host $\leftrightarrow$ Target ConnectRPC Definition (`debug_service.proto`)

ConnectRPC runs over HTTP/2 and supports server and bidirectional streaming, providing typed asynchronous streams for logs, interactive shells, and file transfers.

```protobuf
syntax = "proto3";

package bexos.debug.v1;

// Core Debug Service exposed by target `debugd` over Vsock/TCP/USB
service DebugService {
  // --- Execution & Interactive Shell ---
  rpc OpenStreamSession(stream StreamData) returns (stream StreamData);
  rpc ExecCommand(ExecRequest) returns (ExecResponse);

  // --- Package & App Lifecycle (appd) ---
  rpc InstallPackage(stream PackageChunk) returns (InstallResponse);
  rpc StartApp(StartAppRequest) returns (StartAppResponse);
  rpc StopApp(StopAppRequest) returns (StopAppResponse);
  rpc ListProcesses(ListProcessesRequest) returns (ListProcessesResponse);

  // --- Real-time Logging & Telemetry ---
  rpc StreamLogs(LogFilter) returns (stream LogEntry);
  rpc CaptureCrashDump(CrashDumpRequest) returns (stream CrashDumpChunk);

  // --- Device & Input Automation (E2E Testing) ---
  rpc InjectInput(InputEvent) returns (InjectResponse);
  rpc TakeScreenshot(ScreenshotRequest) returns (ScreenshotResponse);
  rpc Reboot(RebootRequest) returns (RebootResponse);

  // --- State & Storage Manipulation ---
  rpc ResetUserData(ResetUserDataRequest) returns (ResetUserDataResponse);
  rpc SyncFile(stream FileChunk) returns (SyncResponse);
}

// ---------------- Supporting Types ----------------

message StreamData {
  string session_id = 1;
  bytes payload = 2; // PTY stdin/stdout byte stream
  bool is_stderr = 3;
}

message ExecRequest {
  string component_id = 1; // e.g., "com.bexos.browser"
  repeated string args = 2;
  map<string, string> env_overrides = 3;
}

message ExecResponse {
  int32 exit_code = 1;
  string stdout = 2;
  string stderr = 3;
}

message LogFilter {
  uint32 min_severity = 1; // 0=DEBUG, 1=INFO, 2=WARN, 3=ERROR
  string process_filter = 2;
  bool follow = 3;
}

message LogEntry {
  uint64 monotonic_timestamp_ns = 1;
  uint32 pid = 2;
  string component_name = 3;
  string message = 4;
  map<string, string> structured_metadata = 5;
}

message InputEvent {
  oneof event {
    TouchInput touch = 1;
    KeyInput key = 2;
    PointerInput pointer = 3;
  }
}

message TouchInput {
  enum Action { DOWN = 0; MOVE = 1; UP = 2; }
  Action action = 1;
  uint32 x = 2;
  uint32 y = 3;
  uint32 pointer_id = 4;
}

```

### Kernel & System Debug FIDL Interfaces

On the target side, `debugd` holds the capability token `DEBUG_API_ACCESS`. The kernel and `appd` only allow invocations on these privileged debug endpoints if the caller's handle carries that explicit capability bit.

#### A. Kernel Debug Protocol (`bexos.kernel.debug`)

```fidl
library bexos.kernel.debug;

using bexos.kernel;

type ThreadState = strict enum : uint8 {
    RUNNING   = 1;
    SUSPENDED = 2;
    BLOCKED   = 3;
    DEAD      = 4;
};

type ThreadDebugInfo = struct {
    tid uint64;
    name string:32;
    state ThreadState;
    cpu_id uint8;
    rip uint64;
    rsp uint64;
    total_cpu_time_ns uint64;
};

protocol KernelDebugControl {
    /// List all processes and active capability handle counts
    ListProcesses() -> (struct {
        status bexos.kernel.Status,
        process_ids vector<uint64>:256
    });

    /// Suspend a running thread for live inspection / debugger hook
    SuspendThread(struct { tid uint64 }) -> (struct {
        status bexos.kernel.Status
    });

    /// Resume a suspended thread
    ResumeThread(struct { tid uint64 }) -> (struct {
        status bexos.kernel.Status
    });

    /// Read raw process memory region (for GDB/LLDB remote stubs)
    ReadProcessMemory(struct {
        pid uint64,
        vaddr uint64,
        length uint64
    }) -> (struct {
        status bexos.kernel.Status,
        data vector<uint8>:4096
    });

    /// Capture a full snapshot of process page tables and stack state
    CreateCoreDumpVmo(struct { pid uint64 }) -> (resource struct {
        status bexos.kernel.Status,
        core_dump_vmo handle:VMO
    });
};

```

#### B. App & Lifecycle Debug Protocol (`bexos.appd.debug`)

```fidl
library bexos.appd.debug;

using bexos.kernel;

protocol AppDebugControl {
    /// Stage and hot-inject a development .bex package directly into RAM
    InjectTransientPackage(resource struct {
        package_vmo handle:VMO,
        replace_existing bool
    }) -> (struct {
        status bexos.kernel.Status,
        package_id string:128
    });

    /// Force launch an app with isolated debug VMAR and mock capabilities
    LaunchWithOverrides(struct {
        package_id string:128,
        enable_profiling bool,
        tracing_categories vector<string:32>:16
    }) -> (resource struct {
        status bexos.kernel.Status,
        process_handle handle:PROCESS
    });

    /// Take a fast atomic filesystem snapshot for test setup rollback
    CreateSnapshot(struct { snapshot_name string:64 }) -> (struct {
        status bexos.kernel.Status
    });

    /// Roll back to a previously saved filesystem state (instant test reset)
    RestoreSnapshot(struct { snapshot_name string:64 }) -> (struct {
        status bexos.kernel.Status
    });
};

```

### Additional High-Value Features for `debugd`

Beyond basic logging, app install, and shell access, these features will simplify E2E testing:

#### Synthetic Input & Display Injection (No Hardware Required)

* Expose an input injection endpoint that pushes raw multitouch, gamepad, and keyboard events into the BexOS compositor queue.
* Add a `TakeScreenshot()` RPC returning raw framebuffer frames or WebP compressed streams for automated visual regression tests in CI.

#### Capability Graph Visualizer / Inspector

* An RPC (`DumpCapabilityGraph`) that queries `appd` and returns the live capability tree: which apps hold handles to which channels, VMOs, and driver endpoints. This instantly diagnoses capability/resource leaks.

#### Fault Injection & Chaos Testing

* An endpoint to simulate hardware dropouts, NVMe block read errors, dropped network packets, or driver panics to verify that microkernel crash-restart logic and Heart Transplants recover gracefully.

#### Interactive Remote WASM REPL

* Instead of bash, provide a structured REPL that loads a minimal WASI CLI environment or executes dynamic TypeScript/Rust eval expressions against running FIDL endpoints.

### Transport Binding (`debugd` Backend)

`debugd` can listen on multiple transport layers concurrently using the same ConnectRPC handler:

* **In QEMU / Cloud VM:** Listens on `AF_VSOCK` port `9090` (communicates with host test harnesses with near-zero latency).
* **On Real Hardware (USB):** Binds to a USB CDC-NCM or custom USB bulk debug endpoint (similar to Android ADB).
* **Over Local Network:** Binds to `[::]:9090` when local network debugging is toggled on in developer settings.
