# RFC 0027: Userspace network architecture — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0027](README.md)

## Implementation summary

A native VirtIO Ethernet driver and userspace smoltcp stack provide dual-stack sockets and DNS. TLS/HTTP support is in client libraries.

## Implemented behavior

- The D1 VirtIO-Net driver exposes registered frame buffers and Ethernet endpoints. Netstackd manages link configuration, DHCPv4, IPv4/IPv6 addressing, TCP/UDP, listeners, and resolver requests.
- DNS includes secure-resolution configuration; rustls and bounded HTTP/1.1 adapters live in lib/net and distribution rather than the kernel.
- Netstack migration records TCP sequence/window/timer/assembler and bounded buffer state, socket/listener handles, configuration, and link resources. The NIC separately adopts its transport queues.

## Gaps and deviations

- QUIC and HTTP/3 modules currently contain configuration structs, not complete transport implementations. Their presence does not implement the RFC’s protocol matrix.
- DHCPv6, broad NIC/Wi-Fi support, unified VPN/proxy providers, and split-DNS routing are outside the implemented network service.
- TCP migration support does not establish uninterrupted traffic under every network condition or physical NIC handover; secure DNS requires a usable trusted bootstrap path.

## Sources and validation

Implementation and contract evidence: [services/netstack/src/smoltcp_runtime.rs](../../../services/netstack/src/smoltcp_runtime.rs), [services/netstack/src/dns.rs](../../../services/netstack/src/dns.rs), [services/netstack/src/migration.rs](../../../services/netstack/src/migration.rs), [drivers/d1/nic/virtio/net](../../../drivers/d1/nic/virtio/net), [lib/net/src](../../../lib/net/src), [idl/bexos/net/net.fidl](../../../idl/bexos/net/net.fidl).

Relevant test sources and Bazel targets: [services/netstack/tests/netstack_tests.rs](../../../services/netstack/tests/netstack_tests.rs), [lib/net/BUILD.bazel](../../../lib/net/BUILD.bazel), [drivers/d1/nic/virtio/net/BUILD.bazel](../../../drivers/d1/nic/virtio/net/BUILD.bazel).

Detailed guides and previously recorded validation: [services](../../services.md), [drivers storage](../../drivers-storage.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
