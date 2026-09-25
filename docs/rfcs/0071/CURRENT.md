# RFC 0071: Vector packet processing and WASM network extensions — current implementation

- Reviewed: 2026-09-24
- Design: [RFC 0071](README.md)

## Implemented packet plane

`vswitchd` owns forwarding for explicitly routed physical and virtual
interfaces. Reusable packet, graph, routing, neighbor, extension, and control
modules process at most 256 descriptors per invocation. Legacy ports remain
isolated L2 bridges. Routed ingress validates Ethernet/VLAN and endpoint
identity, parses IPv4 or IPv6 with a bounded extension-header walk, performs
per-VRF longest-prefix lookup with metric/interface tie-breaking, decrements
TTL or hop limit, updates IPv4 checksums, enforces MTU, fragments IPv4 when DF
permits, and emits rate-limited ICMP/ICMPv6 errors. Neighbor entries, bounded
unresolved queues, reachability aging, and gateway next hops are native.

The routed hook order is:

`Ethernet → bridge or IP parse → pre-routing NAT → FIB → firewall → post-routing NAT → checksum/MTU → neighbor/output`

Extension calls receive compact routed vectors; L2-only and already-dropped
packets do not enter NAT or firewall. VRF redirects are accepted only when the
target interface belongs to the packet's table. Generation replay and
committed egress continue to bracket the graph.

## Extension sandbox and bundled policy

The version-one no-`std` ABI defines bounded descriptors, vectors, hooks,
configuration, actions, and checkpoint memory. Modules are raw core WASM with
no WASI. The host links only monotonic time, limits module/configuration/memory/
stack/fuel/aggregate state, copies one bounded vector into linear memory, and
copies packet bytes back only for a validated `REWRITE`. A 500 microsecond epoch
watchdog quarantines trapped instances and applies their declared fail policy.
Compilation precedes graph mutation; firewall swaps are single-hook atomic and
NAT pre/post modules are compiled and swapped as one pair. The two NAT hook
instances synchronize one exported bounded checkpoint, so mappings are shared
across outbound and return traffic.

The package embeds Bazel-built recovery modules and also exposes their raw core
WASM targets. The mandatory firewall is compiled and installed before
`vswitchd` reports ready. It implements ordered IPv4/IPv6 zone/direction/
prefix/protocol/port/state rules, established return traffic, bounded
TCP/UDP/ICMP state, deterministic expiry, and fail-closed capacity. Its default
allows virtual/internal egress and established return traffic while denying
unsolicited physical ingress, malformed packets, and unknown fragments.

NAT is disabled until configured. The bundled module provides stateful NAT44
SNAT/PAT, static DNAT/port forwarding, return translation, ICMP echo identifier
translation, bounded fragment association, hairpin traversal, expired-slot reclamation, fail-closed mapping
capacity, and NPTv6 prefix translation with transport checksum adjustment.
NAT64 and stateful NAT66 are not implemented.

## Control, package delivery, and recovery

The private boundary is split into `VirtualSwitchController`,
`SwitchRoutingController`, and `SwitchExtensionController`. Networkd alone
holds these channels and exposes the typed privileged
`NetworkExtensionManager`. Platform prototxt contains routed interfaces,
switch-FIB routes, firewall/NAT policy, and optional signed artifact identity.
Netstack routes remain separate for local socket selection.

Pkgd artifact kind 5 is `NETWORK_EXTENSION`. TUF authenticates the OCI
manifest; pkgd then requires the RFC media types, `vpp-wasm-v1` annotation and
config ABI, exactly one WASM layer, matching descriptor hashes/sizes, and the
expected digest of the returned module layer. Secure grants preserve manifest
provenance and layer digest for authorized cache recovery. Networkd resolves
replacements asynchronously and acknowledges the manager request only after
switch activation. A failed fetch, compile, configuration, or activation keeps
the old graph active. Removing a firewall replacement restores the embedded
firewall; NAT is optional.

Vswitchd migration stores routed interfaces, FIBs, neighbors, unresolved
packets, graph counters and limiter state, module identity/bytes/configuration,
extension counters/fault state, and exported guest checkpoint memory. Networkd
migration stores queued or pending resolutions, resolver channels, module VMO,
and manager caller state. Pending compilation prevents quiescent cutover.

## Validation boundary and future design

Host Bazel tests cover graph routing/batching bounds, L2/VLAN isolation,
IPv4/IPv6 lookup, neighbors and queues, hop/checksum/fragment behavior,
firewall default/state behavior, NAT outbound/return state sharing, OCI media
validation, and service migration codecs. Architecture archive and live-QEMU
results are recorded separately in [testing status](../../testing-status.md).

The RFC's native-JIT, direct VMO/zero-copy extension ABI, NAT64, and stateful
NAT66 designs remain future work. The line-rate, 100 Gb/s, and sub-nanosecond
figures in the design are unverified targets, not measured properties of this
implementation. No benchmark or performance acceptance threshold is added by
this change.
