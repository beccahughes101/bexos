# RFC 0038: System tracing and Perfetto integration

- Created: 2026-08-30T15:12:12-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

Shared producer buffers and traced aggregation capture system events for Perfetto analysis. Debugd and the host test harness expose trace collection, assertions, and CI artifacts.

## Design overview

BexOS uses Perfetto for trace visualization and analysis. The Perfetto UI (`ui.perfetto.dev`) and Trace Processor SQL engine accept native Perfetto protobuf streams and Fuchsia-style trace binary records.

Implementation status (current product): the near-term tracing slice is now
implemented around appd-provisioned shared producer VMOs, traced aggregation,
Perfetto-default export, explicit legacy BexOS FXT export, debugd proxying, and
host trace-analysis assertions. The broader UI/performance goals below remain
the long-term design; no compositor, display stack, GPU path, input service, or
runtime UI producer exists yet.

A centralized D1 **`traced` (Trace Manager)** daemon and per-process zero-copy shared memory buffers provide whole-system microsecond profiling of kernel context switches, FIDL IPC roundtrips, and application rendering loops. Perfetto supplies the visualization interface.

## System Tracing Architecture

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ DEVELOPER / CLI TOOL (`bex trace -d 10s -o trace.perfetto`)                  │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ FIDL: `bexos.tracing.TraceController`
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ TRACE MANAGER DAEMON (`traced` - D1 Userspace Service)                      │
│ • Manages session state (START, STOP, FLUSH) and categories                 │
│ • Allocates physical/paged shared trace VMOs for producers                  │
│ • Aggregates buffer streams into a unified Perfetto/Fuchsia binary file     │
└───────────────▲──────────────────────┬───────────────────────────────▲──────┘
                │                      │                               │
    sys_kcounter/vmo                   │ VMO Shared Memory             │
                │                      ▼                               │
┌───────────────┴────────┐ ┌───────────────────────────┐ ┌─────────────┴──────┐
│ D0 MICROKERNEL         │ │ APPS & USERSPACE SERVICES │ │ D1 DRIVERS (VirtIO) │
│ • Sched context switch │ │ • `appd`, `vfsd`, `net`   │ │ • Hardware IRQs     │
│ • Syscall durations    │ │ • In-process WASM runners │ │ • DMA packet slices │
│ • Page fault latency   │ │ • UI frame timelines      │ │ • Ring buffer drops │
└────────────────────────┘ └───────────────────────────┘ └────────────────────┘

```

## Lock-Free Zero-Copy Shared Memory Ring Buffer

Traces must have sub-microsecond overhead ($< 20\text{ ns}$ per tracepoint). Tracing code **never sends IPC messages per event**.

1. When tracing starts, `traced` creates a shared `VMO` (e.g., 2MB ring buffer) per participating process.
2. `traced` passes the VMO handle to the process over FIDL.
3. The in-process tracing library (`bexos-trace`) writes binary event records (using atomic pointer bumping) directly into this shared VMO:
* **Slices / Scopes:** Function entries and exits (`BEGIN_SLICE`, `END_SLICE`).
* **Instant Events:** One-off alerts (e.g., `CACHE_FLUSH`).
* **Flows:** Track causality across IPC boundaries (linking caller IPC send to server handling).
* **Counters:** Memory allocations, active handles, thread counts.

## D0 Microkernel Trace Instrumentation

The microkernel exports lightweight scheduler and hardware counters via a dedicated kernel trace buffer:

* **Scheduler Switch Events (`sched_switch`):** Captures which thread and process ID are running on which CPU core.
* **Syscall Tracing:** Records entry/exit timestamps for microkernel syscalls like `sys_channel_call`.
* **Hardware Interrupts:** Captures hardware IRQ latency before yielding to userspace driver hosts.

## Trace Producer & Controller FIDL (`bexos.tracing`)

```fidl
library bexos.tracing;

using bexos.kernel;

type BufferMode = strict enum : uint8 {
    ONESHOT_STOP_ON_FULL = 1;
    CIRCULAR_RING        = 2;
};

type TraceCategory = strict bits : uint32 {
    KERNEL_SCHED   = 0x0001;
    IPC_MESSAGES   = 0x0002;
    VFS_IO         = 0x0004;
    NETWORK_STACK  = 0x0008;
    UI_FRAMES      = 0x0010;
    APP_CUSTOM     = 0x0020;
};

/// Interface used by CLI tools (e.g. `bex trace`) to control sessions
@discoverable
protocol TraceController {
    StartSession(struct {
        categories TraceCategory;
        buffer_mode BufferMode;
        buffer_size_kb uint32;
    }) -> (struct {
        status bexos.kernel.Status;
    });

    StopSession() -> (resource struct {
        status bexos.kernel.Status;
        trace_file handle:VMO; // Emits unified Perfetto-compatible protobuf or Fuchsia trace stream
    });
};

/// Interface registered by Apps and Drivers with `traced`
@discoverable
protocol TraceSink {
    RegisterProducer(resource struct {
        process_name string:64;
        buffer handle:VMO;
        categories TraceCategory;
    }) -> (struct {
        status bexos.kernel.Status;
    });
};

```

## In-Process Rust / WASM Macro API (`bexos-trace`)

Services, drivers, and application runtimes use lightweight RAII scopes for instrumentation:

```rust
use bexos_trace::{trace_scope, trace_instant, trace_flow_begin};

pub fn handle_netstack_packet(packet_id: u64, data: &[u8]) {
    // Automatically writes BEGIN_SLICE to shared VMO, emits END_SLICE on drop (~15ns)
    trace_scope!("netstack:handle_packet", "len" => data.len() as u64);

    if is_syn_flood(data) {
        trace_instant!("netstack:syn_flood_detected");
    }

    // Link packet causality across into the app process
    trace_flow_begin!("ipc_flow", packet_id);
}

```

## Viewing the Trace

Because Perfetto's UI ([https://ui.perfetto.dev](https://ui.perfetto.dev)) natively supports Fuchsia trace binary format and Perfetto Protobuf:

1. Run CLI: `bex trace --duration 5s -o session.trace`
2. Drag and drop `session.trace` directly into `ui.perfetto.dev`.
3. The timeline shows:
* **CPU Cores:** Exact timeline of thread scheduling and context switches.
* **IPC Flows:** Arrows linking a client FIDL call across to `vfsd` or `netstack` down to the hardware driver.
* **App Rendering:** UI frame pipelines, VSYNC drops, and WASM fuel consumption.

The Rust QEMU E2E harness integrates Perfetto to assert trace properties, such as IPC latency budgets and execution of specific FIDL flows. Captured timelines support post-test debugging of CI failures.

## End-to-End Test & Tracing Flow

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ HOST MACHINE (Cargo Test Runner)                                            │
│                                                                             │
│  [ Rust E2E Test: `tests/storage_stress.rs` ]                               │
│     │                                                                       │
│     │ 1. Spawns QEMU with serial/socket redirection                         │
│     │ 2. Sends command to `debugd`: `TRACE_START categories=VFS,KERNEL`     │
│     │ 3. Executes test scenario (e.g. installs package, writes file)        │
│     │ 4. Sends command to `debugd`: `TRACE_STOP`                            │
│     ▼                                                                       │
│  [ Debugd Stream Transport (QEMU Serial / VirtIO-Console) ]                 │
│     │                                                                       │
│     │ 5. Streams Perfetto/Fuchsia binary trace bytes back to Host           │
│     ▼                                                                       │
│  [ Perfetto Trace Processor SQL Engine / Parser ]                           │
│     │                                                                       │
│     ├─► 6. Programmatic Assertions:                                         │
│     │      `SELECT max(dur) FROM slice WHERE name='vfsd:write'` < 500µs     │
│     └─► 7. Saves `target/traces/<test_name>.perfetto` for CI UI inspection  │
└─────────────────────────────────────────────────────────────────────────────┘

```

## `debugd` Protocol Extension

Extend `debugd` to expose tracing controls over the serial/QEMU pipe or VirtIO-console back to the host:

```fidl
library bexos.debug;

using bexos.kernel;
using bexos.tracing;

@discoverable
protocol DebugTraceBridge {
    /// Tell traced to start recording
    StartTrace(struct {
        categories bexos.tracing.TraceCategory;
        buffer_size_kb uint32;
    }) -> (struct {
        status bexos.kernel.Status;
    });

    /// Stop recording and stream the resulting trace data back over a kernel socket
    StopTraceAndStream() -> (resource struct {
        status bexos.kernel.Status;
        stream handle:SOCKET;
    });
};

```

## Rust Host Test Harness (`bexos-e2e-harness`)

Create an RAII guard in the Rust test harness that automatically starts tracing, runs the test body, captures the trace on test completion or failure, and saves the file.

```rust
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

pub struct TracedTestSession {
    qemu: QemuInstance,
    test_name: String,
}

impl TracedTestSession {
    pub async fn start(test_name: &str, categories: &str) -> Self {
        let qemu = QemuInstance::spawn_and_connect_debugd().await;

        // Instruct debugd inside BexOS to start recording
        qemu.debugd_cmd(&format!("trace start {}", categories)).await;

        Self {
            qemu,
            test_name: test_name.to_string(),
        }
    }

    /// Stop tracing, pull the trace bytes, and return a trace analyzer
    pub async fn stop_and_collect(self) -> TraceAnalysis {
        let trace_bytes = self.qemu.debugd_stream_cmd("trace stop").await;

        // Save trace file for local inspection at ui.perfetto.dev or CI artifact upload
        let trace_path = PathBuf::from(format!("target/traces/{}.perfetto", self.test_name));
        std::fs::create_dir_all("target/traces").unwrap();
        let mut file = File::create(&trace_path).unwrap();
        file.write_all(&trace_bytes).unwrap();

        TraceAnalysis::load(trace_path)
    }
}

```

## Writing E2E Tests with Perfetto Trace Assertions

SQL queries through the Perfetto `trace_processor` binary assert functional behavior and performance budgets against the collected trace:

```rust
#[tokio::test]
async fn test_monzo_app_launch_performance() {
    // 1. Start test session with tracing active
    let session = TracedTestSession::start(
        "test_monzo_app_launch_performance",
        "APP_LIFECYCLE,VFS,IPC,KERNEL_SCHED"
    ).await;

    // 2. Perform the action inside BexOS via debugd
    session.qemu.run_shell("appd-ctl launch com.monzo:monzo").await;
    session.qemu.wait_for_log("monzo initialized", std::time::Duration::from_secs(5)).await;

    // 3. Stop and extract the trace
    let trace = session.stop_and_collect().await;

    // 4. Assert on trace slices using Perfetto SQL queries

    // Assertion A: App launch must complete within 200ms
    let launch_dur_ms: f64 = trace.query_scalar(
        "SELECT dur / 1e6 FROM slice WHERE name = 'appd:spawn_process' AND track_name = 'appd'"
    ).await;
    assert!(launch_dur_ms < 200.0, "App launch took too long: {}ms", launch_dur_ms);

    // Assertion B: Ensure zero page-fault hangs or excessive IPC wait times
    let max_vfs_read_us: f64 = trace.query_scalar(
        "SELECT max(dur) / 1e3 FROM slice WHERE name = 'vfsd:archivefs_read_chunk'"
    ).await;
    assert!(max_vfs_read_us < 2000.0, "VFS chunk read spiked: {}us", max_vfs_read_us);

    // Assertion C: Verify expected execution flow occurred
    let flow_count: i64 = trace.query_scalar(
        "SELECT count(*) FROM flow WHERE name = 'app_startup_to_first_frame'"
    ).await;
    assert_eq!(flow_count, 1, "Missing startup flow linkage");
}

```

## CI Artifact Pipeline

When running in automated CI (e.g., GitHub Actions):

* **Passing tests:** Traces are discarded or kept for regression tracking.
* **Failing tests:** The harness automatically attaches `target/traces/<failed_test>.perfetto` to the CI build summary.
* **Instant Triage:** Developers click the artifact URL, open `ui.perfetto.dev`, and immediately see the exact kernel context switches, lock contentions, or blocked IPC channels that caused the test to fail or hang.
