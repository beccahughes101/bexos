# RFC 0056: BexOS Workplace remote sessions — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0056](README.md)

## Implementation summary

BexOS has local graphics, user isolation, and network primitives, but no Workplace remote-session product is implemented.

## Implemented behavior

- Scened provides local composition and input control; usersd/vfsd provide authentication and encrypted user storage.
- Netstack supplies TCP/UDP and client TLS support. Secure-monitor code isolates its particular normal/secure domains; it is not a multi-tenant Workplace session orchestrator.

## Gaps and deviations

- No remote desktop session broker, AV1/HEVC streaming encoder pipeline, WebRTC/QUIC remote transport, browser client, or native thin-client package matching this RFC was found.
- Remote USB/peripheral redirection, enterprise identity enrollment, ephemeral tenant overlays, and session migration/heart transplant are not implemented end to end.
- The density, RAM, latency, and licensing/economics claims are proposals without workload measurements for an actual Workplace deployment. Generic graphics tests do not validate them.

## Sources and validation

Implementation and contract evidence: [services/scened/src](../../../services/scened/src), [services/usersd/src](../../../services/usersd/src), [services/vfsd/src](../../../services/vfsd/src), [services/netstack/src](../../../services/netstack/src), [lib/net/src/quic.rs](../../../lib/net/src/quic.rs), [secure/monitor/runtime](../../../secure/monitor/runtime).

No dedicated implementation test for this RFC was found in the reviewed tree.

Detailed guides and previously recorded validation: [scened](../../scened.md), [services](../../services.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
