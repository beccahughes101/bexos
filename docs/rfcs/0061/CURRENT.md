# RFC 0061: Unified egress and split-DNS routing — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0061](README.md)

## Implementation summary

The current service is netstackd with separate IP socket and hostname-resolution operations. Unified outbound providers and split DNS are not implemented.

## Implemented behavior

- Netstack.ConnectTcp accepts a SocketAddress and a server endpoint; ResolveHost is a separate call returning IP addresses. TCP/UDP and configured DNS/dual-stack routing are implemented.
- Existing netstack and VirtIO-Net migration preserve supported socket/link state; they do not register or migrate external VPN/proxy providers.

## Gaps and deviations

- The proposed domain-or-IP Endpoint socket contract, NetworkRoutingManager, RegisterEgressProvider, and ProxyStreamHandler were not found in current IDL or runtime code.
- TUN/WireGuard/IPsec egress, SOCKS5/HTTP CONNECT delegation, per-provider DNS and suffix routing, and proxy-side hostname privacy remain unimplemented.
- No redirectord service or ResolveAlias contract implements go/ short links, HTTP redirects, encrypted alias storage, or synchronization. Existing prefsd and DNS do not provide those features automatically.

## Sources and validation

Implementation and contract evidence: [idl/bexos/net/net.fidl](../../../idl/bexos/net/net.fidl), [services/netstack/src/service.rs](../../../services/netstack/src/service.rs), [services/netstack/src/dns.rs](../../../services/netstack/src/dns.rs), [services/netstack/src/config.rs](../../../services/netstack/src/config.rs), [services/netstack/src/migration.rs](../../../services/netstack/src/migration.rs), [lib/net_client](../../../lib/net_client).

No dedicated implementation test for this RFC was found in the reviewed tree.

Detailed guides and previously recorded validation: [services](../../services.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
