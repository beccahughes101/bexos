# Tracing

## Current Implementation

BexOS tracing now uses appd-provisioned per-process producer VMOs plus a
central `traced` aggregator:

- `//lib/trace` provides a no-std event model, category helpers, RAII scopes,
  instant events, counters, flows, a lock-free shared-VMO writer/reader, native
  Perfetto protobuf export, and explicit legacy BexOS FXT export.
- appd starts processes with startup ABI v8 and an optional trace producer
  descriptor containing a 2 MiB VMO, mapped length, stable producer ID, PID, and
  main TID. Startup decoding remains compatible with v2-v6.
- `//services/traced` exposes public `TraceController` for session control and
  privileged `TraceRegistry` for producer VMO register/replace/unregister.
- `debugd`, `//host/debug_client`, and `//tools/bexctl` proxy trace control to
  `traced` and keep debug-wire chunking unchanged.
- `//host/trace_analysis` and `//tools/trace_analysis` provide host-side trace
  assertion/query helpers. SQL queries run through a configured Perfetto Trace
  Processor binary via `BEXOS_TRACE_PROCESSOR`.

Perfetto protobuf (`.pftrace`) is the default output. The old
`FXT\0BEXOS\0` encoder remains as `LEGACY_BEXOS_FXT` and is intentionally
described as a private legacy BexOS format, not native Fuchsia FXT.

## Producer Buffers

Each native process can receive one appd-created trace VMO. The shared layout is:

- one page header with magic/version, session epoch, enabled categories, buffer
  mode, active byte length, slot count, write sequence, and dropped count;
- fixed 160-byte committed slots containing timestamp, PID/TID, category, event
  kind, event name, flow/counter value, and two numeric annotations;
- atomic reservation/commit so concurrent writers can publish without a lock.

`buffer_size_kb` selects the active prefix of the 2 MiB VMO. Circular mode
overwrites old slots and accounts drops. Oneshot mode stops committing after the
active prefix fills and increments the dropped counter.

During heart transplant, `traced` preserves registered producer mappings/read
cursors and appd keeps using stable producer IDs. Replacement processes receive
fresh producer buffers and are registered through `TraceRegistry`.

## Categories

Trace categories are bit flags shared by `//lib/trace`, tracing FIDL, debug
wire, host client calls, and CLI options:

- `kernel` or `kernel_sched`
- `ipc` or `ipc_messages`
- `vfs` or `vfs_io`
- `net`, `network`, or `network_stack`
- `ui` or `ui_frames`
- `app` or `app_custom`
- `debug`, `service`, or `debug_service`
- `all`

The shorthand macros use `app_custom`. Category-explicit macro forms are the
canonical API for subsystem instrumentation.

## Collect From QEMU

Start the QEMU developer product:

```sh
bazel run //device/virtual/qemu/nongui:run
```

From another terminal, record a five-second Perfetto trace:

```sh
bazel run //device/virtual/qemu/nongui:debugd -- trace record --duration-ms 5000 -o /tmp/bexos.pftrace
```

Manual control is also available:

```sh
bazel run //device/virtual/qemu/nongui:debugd -- trace start --categories debug,ipc --buffer-size-kb 2048 --format perfetto
bazel run //device/virtual/qemu/nongui:debugd -- ps
bazel run //device/virtual/qemu/nongui:debugd -- trace status
bazel run //device/virtual/qemu/nongui:debugd -- trace stop -o /tmp/bexos.pftrace
```

Use the explicit legacy format only for compatibility:

```sh
bazel run //device/virtual/qemu/nongui:debugd -- trace record --format fxt --duration-ms 5000 -o /tmp/bexos.fxt
```

`trace stop` uses chunked debug-wire responses, so callers do not need to know
the final trace size ahead of time.

## Host Client And Analysis APIs

`//host/debug_client` exposes typed helpers:

```rust
client.trace_start(bexos_trace::CATEGORY_DEBUG_SERVICE, 2, 2048)?;
client.trace_start_with_format(
    bexos_trace::CATEGORY_DEBUG_SERVICE,
    bexos_trace::BufferMode::CircularRing.to_wire(),
    2048,
    bexos_trace::TraceOutputFormat::LegacyBexosFxt.to_wire(),
)?;
let status = client.trace_status()?;
let bytes = client.trace_stop()?;
```

`//host/trace_analysis` can run reusable assertions and, when
`BEXOS_TRACE_PROCESSOR` points at `trace_processor_shell`, arbitrary Perfetto
SQL:

```rust
let analysis = bexos_trace_analysis::TraceAnalysis::from_file("/tmp/bexos.pftrace")?;
analysis.assert_event_present("debugd:health_check")?;
analysis.assert_category_present("debug_service")?;
let slices = analysis.scalar_i64("select count(*) from slice")?;
```

CLI form:

```sh
bazel run //tools/trace_analysis -- /tmp/bexos.pftrace event debugd:health_check
bazel run //tools/trace_analysis -- /tmp/bexos.pftrace query "select name, dur from slice limit 10"
```

## E2E Helper API

`testing/e2e::DebugSession` defaults to Perfetto and `.pftrace` artifacts:

```rust
let mut traced = session.traced_test("debugd_health", bexos_trace::CATEGORY_DEBUG_SERVICE)?;
traced.session().assert_debugd_ready()?;
let analysis = traced.finish()?;
analysis.assert_event_present("debugd:health_check")?;
```

When running under Bazel, the default guard path is
`$TEST_UNDECLARED_OUTPUTS_DIR/<test>.pftrace`. Outside Bazel, it falls back to
`target/traces/<test>.pftrace`.

## Instrumentation Coverage

Implemented tracepoints currently cover:

- kernel syscall entry, scheduler timer/IPI decisions, IPC channel send/read/call
  flows, IRQ counters, and sync-fault/page-fault style exception records;
- appd startup waves, launch/readiness, capability binding, worker launch, and
  replacement phases through existing appd trace macros and the v7 producer
  provisioning path;
- vfsd request dispatch, archive/package/mount-style operations, counters, and
  failures;
- netstack control/data-plane polling, request ordinals, queue activity, and
  RX/TX style packet work;
- debugd requests and traced session/producer lifecycle.

`CATEGORY_UI_FRAMES` is fully supported by the shared writer and Perfetto
exporter. Future UI services should use event names such as
`ui:frame_begin`, `ui:frame_submit`, `ui:frame_present`,
`ui:frame_missed_deadline`, and `ui:frame_dropped`. The current product still
has no UI, display, GPU, input, shell, or compositor runtime producer.

## Current Limitations

- Perfetto SQL execution depends on a configured `BEXOS_TRACE_PROCESSOR`
  executable. The host assertion API works without it for event/category
  presence checks.
- The product still has no UI stack, so `ui_frames` has no runtime producer.
- QEMU/e2e trace validation exists but is not part of the non-e2e verification
  commands for this change.
