# RFC 0027: Userspace network architecture — current implementation

- Reviewed: 2026-09-24
- Design: [RFC 0027](README.md)

## Implementation summary

A native Ethernet driver, an isolated virtual switch, RFC 68 network front ends,
and table-scoped smoltcp back ends provide dual-stack sockets and split DNS.
TLS/HTTP support remains in client libraries and secure resolver transports.

## Implemented behavior

- D1 Ethernet drivers expose registered frame buffers to vswitchd. Vswitchd gives each netstack table an isolated descriptor-ring virtual port; netstackd no longer receives the product's physical NIC directly.
- Networkd publishes domain-scoped SocketProvider channels, owns RFC 61 routing/resolution and socket-control escrow, and leaves TCP payload bytes on kernel socket handles.
- Netstackd manages per-table FIB/interface/link, DHCP/runtime, IPv4/IPv6, TCP/UDP, listeners, quotas, and ephemeral ports.
- DNS includes secure-resolution configuration; rustls and bounded HTTP/1.1 adapters live in lib/net and distribution rather than the kernel.
- Networkd, netstackd, vswitchd, and the NIC are heart-transplant participants. Netstack recovery journals and networkd endpoint escrow add crash adoption to the planned migration path.

## Gaps and deviations

- QUIC and HTTP/3 modules currently contain configuration structs, not complete transport implementations. Their presence does not implement the RFC’s protocol matrix.
- DHCPv6 and broad physical NIC/Wi-Fi support remain future work.
- VPN/proxy registration is implemented, but WireGuard/IPsec/SOCKS5/HTTP-CONNECT engines are not bundled.
- Secure DNS requires an explicit reachable bootstrap address and a valid trustd root path.

## Sources and validation

Implementation and contract evidence: [services/networkd](../../../services/networkd), [services/netstack](../../../services/netstack), [services/vswitchd](../../../services/vswitchd), [drivers/d1/nic/virtio/net](../../../drivers/d1/nic/virtio/net), [lib/net/src](../../../lib/net/src), [idl/bexos/net/net.fidl](../../../idl/bexos/net/net.fidl).

Relevant test sources and Bazel targets: [services/netstack/tests/netstack_tests.rs](../../../services/netstack/tests/netstack_tests.rs), [lib/net/BUILD.bazel](../../../lib/net/BUILD.bazel), [drivers/d1/nic/virtio/net/BUILD.bazel](../../../drivers/d1/nic/virtio/net/BUILD.bazel).

Detailed guides and previously recorded validation: [services](../../services.md), [drivers storage](../../drivers-storage.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
