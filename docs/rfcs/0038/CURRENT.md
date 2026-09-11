# RFC 0038: System tracing and Perfetto integration — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0038](README.md)

## Implementation summary

Shared producer buffers, traced aggregation, Perfetto export, and host collection/assertion helpers are implemented.

## Implemented behavior

- Appd provisions trace VMOs and producer identity in startup metadata. Lib/trace writes bounded records, and traced registers producers and controls sessions.
- Kernel syscall/scheduler/IPC/fault and core service instrumentation feed exports. Debugd proxies collection; bexctl and host helpers retrieve trace artifacts.
- Perfetto protobuf is the default output; the custom legacy BexOS FXT encoding is explicitly selectable. Traced has a migration adapter for its service/session state.

## Gaps and deviations

- The RFC’s statement that no compositor/display/input exists is stale, but those new graphics components do not automatically become lib/trace runtime producers. A complete ui_frames instrumentation path was not found in the reviewed graphics sources.
- Perfetto SQL assertions require a configured trace-processor executable; basic event/category checks do not.
- Presence of tracepoints and test helpers does not establish loss-free zero-copy export, low tracing overhead, or a deployed CI performance gate.

## Sources and validation

Implementation and contract evidence: [lib/trace](../../../lib/trace), [services/traced](../../../services/traced), [kernel/src/tracing.rs](../../../kernel/src/tracing.rs), [services/appd/src/guest/trace_registry.rs](../../../services/appd/src/guest/trace_registry.rs), [services/debugd/src/service/trace.rs](../../../services/debugd/src/service/trace.rs), [tools/trace_analysis](../../../tools/trace_analysis).

Relevant test sources and Bazel targets: [lib/trace/BUILD.bazel](../../../lib/trace/BUILD.bazel), [services/traced/BUILD.bazel](../../../services/traced/BUILD.bazel), [tools/trace_analysis/BUILD.bazel](../../../tools/trace_analysis/BUILD.bazel).

Detailed guides and previously recorded validation: [tracing](../../tracing.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
