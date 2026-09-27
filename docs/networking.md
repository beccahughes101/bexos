# Networking architecture

The networking stack separates local transport termination from packet
forwarding. Applications use a domain-scoped `SocketProvider` published by
networkd. Netstackd owns TCP/UDP, DHCP/SLAAC clients, DNS, application sockets,
and the per-table routes used to choose a local socket interface. Vswitchd owns
the physical NIC, virtual descriptor rings, legacy L2 bridge isolation, and the
separate switch FIB used only for forwarding.

## Starnix workload networking

Linux workloads are offline unless their manifest or durable container spec
names one or more platform `workload_profiles`. Appd authenticates the package
signer or verified OCI manifest digest, and networkd repeats that check before
provisioning. A direct profile yields a provider permanently scoped to its RFC
68 domain/table. A virtual-L2 profile yields a leased vswitch port, locally
administered MAC, bounded rings, MTU, addresses, routes, and DNS configuration.
All attachments for one launch succeed or fail together.

The ordered attachments become `eth0`, `eth1`, and so on unless explicit Linux
interface names are supplied. Linux can alter only its namespace-local links,
addresses, routes, veth pairs, bridges, and firewall/NAT state. Physical
selectors, VLANs, host routes, provider domains, and raw/promiscuous grants
remain platform policy. Closing the lease removes a dynamic port; reboot
recreates it from the durable workload spec rather than treating dynamic
topology as authoritative configuration.

Each Linux network namespace owns its FIB, connection tracking, and firewall
generation. `NETLINK_NETFILTER` batches stage privately and publish only at the
batch terminator. The supported nft expression subset covers interface,
address, protocol, port, and connection-state matches; counters; terminal
accept/drop/reject verdicts; and bounded SNAT, DNAT, and masquerade. Legacy
IPv4 `IPT_SO_SET_REPLACE` rules using the corresponding standard matches and
targets translate into the same engine. Unknown extensions return
`EOPNOTSUPP` without changing the active ruleset. Direct-provider interfaces
can filter flows but reject packet-level NAT because their traffic does not
traverse the namespace packet plane.

## Routed packet path

A port stays in its legacy isolated bridge mode unless platform prototxt names
it in a routed-interface policy. A routed interface binds exactly one physical
or virtual endpoint to a VRF table, bridge domain, VLAN, MAC, MTU, address set,
and security zone. Networkd programs those interfaces and switch routes through
the private `SwitchRoutingController`; it programs netstackd's local routes
separately.

Vswitchd accepts at most 256 descriptors per graph invocation. It validates
Ethernet and VLAN identity, parses IPv4 or bounded IPv6 extension headers, and
groups the active routed packets through this order:

`Ethernet → bridge or IP parse → pre-routing NAT → FIB → firewall → post-routing NAT → checksum/MTU → neighbor/output`

Native nodes own L2 demux, per-VRF longest-prefix lookup, gateway selection,
ARP/NDP neighbor state and bounded unresolved queues, TTL/hop-limit updates,
IPv4 header checksums and fragmentation, MTU enforcement, and rate-limited
ICMP/ICMPv6 errors. Generation-based ingress replay and committed egress wrap
the graph, so a netstackd recovery cannot release uncommitted traffic.

## Network extensions

Firewall and NAT policy execute as sandboxed raw core-WASM modules inside
vswitchd. Their no-`std` ABI copies bounded packet vectors through guest linear
memory and offers `DROP`, `PASS`, `REWRITE`, and VRF-checked `REDIRECT` actions.
No WASI services, filesystem, sockets, or arbitrary handles are available.
Fuel, memory/stack/configuration/state limits and a 500 microsecond epoch
watchdog bound execution. A trap quarantines the instance and applies its
declared fail policy.

The embedded firewall is fail closed and is installed before readiness. Its
default allows internal/virtual egress and established return traffic while
denying unsolicited external ingress. NAT is opt-in and supplies stateful
NAT44/PAT, static forwarding and hairpinning, ICMP identifiers, and NPTv6.
NAT64 and stateful NAT66 are not implemented.

Networkd is the only caller of the private switch controllers. Its public
privileged `NetworkExtensionManager` resolves desired signed artifacts through
pkgd, waits for switch activation, and then replies. Pkgd verifies the signed
OCI manifest, exact RFC media types and `vpp-wasm-v1` ABI, and a single raw-WASM
layer. Boot starts from embedded modules so package resolution never blocks the
network path. Removing a signed firewall replacement restores the embedded
firewall; optional NAT can be removed.

## Recovery and authoritative state

Vswitchd heart transplant includes device mappings, bridge queues and packet
generations, routed interfaces and FIBs, neighbors and unresolved packets,
graph counters/limiters, module identity and configuration, counters/faults,
and exported guest checkpoint memory. Networkd retains manager clients, queued
deployments, pending pkgd resolver channels, and the resolved module VMO while
activation is pending.

Dynamic changes survive heart transplant but not reboot. Authored product
configuration remains prototxt and is authoritative after reboot. Future
native-JIT, direct VMO/zero-copy extension access, NAT64, and performance
targets remain documented in the RFCs; no line-rate or sub-nanosecond result is
claimed by the current implementation.
