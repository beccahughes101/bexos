# RFC 0068: isolated network instances and virtual switching — current implementation

- Reviewed: 2026-09-24
- Design: [RFC 0068](README.md)

## Implemented architecture

The application path is now `application → domain-scoped networkd → netstackd
VRF table → vswitchd virtual port → physical Ethernet device`. Appd launches
the configured networkd and netstackd instance for each isolation group in a
separate process and resource job. Managed-service identity, watchdog state,
updates, cleanup, published-provider metadata, and migration records include the
instance identity. Platform boot policy is prototxt and defines isolation
groups, resource templates and limits, named domains, tables, physical
selectors, virtual ports, MAC/VLAN policy, routes, and DNS upstreams.

Networkd owns the public compatibility `Netstack`, domain-scoped
`SocketProvider`, privileged `NetworkRoutingManager`, TCP/listener/UDP control
proxies, RFC 61 provider selection, resolver policy, and backend escrow. The
legacy ordinals 1–6 remain unchanged and are published only by the default
networkd instance for `system_default`. App manifests may select
`Process.network_domain`; omission selects `system_default`, while an unknown,
unauthorized, or filter-conflicting domain fails launch.

Netstackd is a router containing independent table state: FIB, interfaces,
packet links, neighbors/runtime generations, TCP, listeners, UDP, DNS fallback,
quotas, and ephemeral ports. An interface belongs to one table, route lookup is
longest prefix with metric/interface tie-breaking, listeners attach only to an
eligible interface, and UDP routing is table-local. Private backend and
controller bindings are accepted only from networkd. The normal product no
longer grants the physical Ethernet capability directly to netstackd.

Vswitchd exclusively consumes physical Ethernet devices and exposes isolated
virtual descriptor-ring devices. Legacy ports retain destination MAC/VLAN
demux, broadcast and multicast fan-out, access/trunk VLAN translation, source
MAC/VLAN anti-spoofing, bounded queues, backpressure, and a bounded copy between
the physical and per-port VMO pools. Explicitly routed endpoints additionally
enter the RFC 71 native L2/L3 vector graph: per-VRF FIB lookup, neighbor state,
hop-limit/checksum processing, IPv4 fragmentation, MTU/ICMP errors, and
sandboxed firewall/NAT hooks. Physical and virtual devices, mappings, port
policy, routed interfaces, FIBs, neighbors and queued packets, extension state,
counters, and packet generations participate in heart transplant. The QEMU
`qemu-default` selector resolves to the sole enumerated NIC.

## Recovery behavior

Networkd retains the application control channels, kernel stream endpoint, and
duplicate backend/control endpoints for every direct TCP flow, listener, and
UDP socket. Netstackd produces versioned, architecture-tagged, checksummed,
double-slot recovery journals. Networkd checkpoints each backend by generation,
keeps the last committed journal in its own transplant state, detects a fresh
backend through the minimum-generation contract, adopts the journal, and then
reattaches the escrowed stream/listener/UDP handles. Corrupt, stale,
cross-generation, or wrong-architecture journals fail closed.

Vswitchd stages outbound frames by generation and commits them only after
networkd has a matching backend checkpoint. Delivered inbound frames remain in
a generation journal until the same commit. On backend recovery, vswitchd
requeues delivered ingress newer than the adopted journal and discards staged
egress newer than it, preventing loss, duplicate commit, or cross-generation
release. Its migration records retain queued ingress, delivered ingress, and
staged outbound frames. Planned replacement additionally uses the existing
incremental heart-transplant records for all three services, including packet
link mappings and smoltcp TCP checkpoint state.

## Product and compatibility status

Networkd and vswitchd package/replacement archives are assembled into both QEMU
architectures. The platform policy provisions table zero, one virtual port, a
default route, and QEMU DNS. Libc, `lib/net`, WASM sockets, pkgd, timed, jobd,
and the ELF network fixture prefer `SocketProvider` and retain explicit legacy
fallback. The network transplant fixture now replaces the VirtIO NIC,
vswitchd, netstackd, and networkd while its kernel socket remains open.

Runtime provider/table state and recovery journals survive service replacement
but are intentionally not persisted across reboot. Boot prototxt remains the
authoritative reboot topology.

Networkd is also the sole client of the private switch routing and extension
controllers. It restores pending package-resolution channels and deployments
during replacement. Boot always starts with the embedded fail-closed firewall;
optional NAT and verified signed replacements are applied only after the
network path is available, avoiding a network/pkgd dependency cycle.

## Deliberate exclusions and validation boundary

The privileged stack/switch controllers remain discoverable for assembly but
require `BEXOS_SYSTEM_PRIVILEGED` at bind time. The typed proxy and
device-provider surfaces are implemented; SOCKS5, HTTP
CONNECT, WireGuard, and IPsec protocol engines are not part of this RFC 0068
implementation. Redirectord and short-link storage/HTTP behavior remain outside
scope. Host tests cover routing, DNS parsing/cache isolation, VRF isolation,
virtual switching, migration codecs, journal validation, and appd lifecycle
state. The RFC 71 additions also have host coverage for routed graph behavior,
bundled firewall/NAT behavior, package verification, and extension migration.
QEMU live/crash recovery, routed policy, and split-DNS scenarios must only be claimed
after their architecture-specific guest targets pass; see
[testing status](../../testing-status.md).

## Source map

- Public/private networking contracts: [net.fidl](../../../idl/bexos/net/net.fidl)
- App and platform schemas: [manifest.proto](../../../idl/bexos/app/manifest.proto), [config.proto](../../../idl/bexos/platform/config.proto)
- Front end: [networkd](../../../services/networkd)
- VRF backend: [netstack](../../../services/netstack)
- Virtual switch: [vswitchd](../../../services/vswitchd)
- Instance orchestration: [appd](../../../services/appd)
