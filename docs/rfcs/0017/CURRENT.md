# RFC 0017: Host bridge and remote debugging — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0017](README.md)

## Implementation summary

Bexctl, the host debug client, and debugd implement the framed BXD1 QEMU socket bridge with typed control and interactive terminal support.

## Implemented behavior

- The host client frames and bounds requests; debugd proxies app lifecycle, users, configuration, tracing, TEE, update, and diagnostic operations to owning services.
- Interactive sessions authenticate separately and launch the configured Brush/WASM shell over explicit streams. Diagnostic ExecCommand remains a constrained command surface.
- Debugd migration retains transport/upload and terminal state with bounded quiescence, rather than opening a new host connection after each replacement.

## Gaps and deviations

- The proposed ConnectRPC/HTTP2 transport, general network/USB transports, log streaming, capability graph UI, and remote WASM development REPL are not the implemented bridge.
- QEMU sockets are a development transport; their presence does not establish a production remote-debug authentication/deployment policy.
- Continuity is bounded by supported protocol/session state and migration tests, not arbitrary host connections or unmeasured zero downtime.

## Sources and validation

Implementation and contract evidence: [lib/debug_wire](../../../lib/debug_wire), [host/debug_client/src](../../../host/debug_client/src), [services/debugd/src/service.rs](../../../services/debugd/src/service.rs), [services/debugd/src/transport.rs](../../../services/debugd/src/transport.rs), [services/debugd/src/live_migration.rs](../../../services/debugd/src/live_migration.rs), [tools/bexctl/src](../../../tools/bexctl/src).

Relevant test sources and Bazel targets: [services/debugd/tests](../../../services/debugd/tests), [host/debug_client/tests](../../../host/debug_client/tests), [tools/bexctl/BUILD.bazel](../../../tools/bexctl/BUILD.bazel).

Detailed guides and previously recorded validation: [cli](../../cli.md), [brush shell](../../brush-shell.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
