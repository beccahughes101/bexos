# RFC 0061: unified egress and split-DNS routing — current implementation

- Reviewed: 2026-09-24
- Design: [RFC 0061](README.md)

## Implemented behavior

`bexos.net` now includes typed domain/IP `Endpoint`, `TableId`, IP prefixes,
domain suffixes, DNS transports/upstreams, device/proxy targets,
`SocketProvider`, `NetworkRoutingManager`, and `ProxyStreamHandler`. The legacy
`Netstack` ordinals 1–6 are wire-compatible and confined to `system_default`.
Applications receive a provider already bound to their appd-selected domain;
they never submit a table identifier.

Networkd normalizes ASCII DNS names and selects domain routes on label
boundaries by longest suffix, then lowest priority, then stable provider token.
IP routes use longest prefix with the same deterministic ties. Existing flows
pin provider token and generation. Removal stops new flows, waits for pinned
flows before deleting a provider/table, and a matched but unavailable suffix is
fail-closed rather than falling back to public DNS or another egress.

Proxy routes receive the normalized original hostname through
`ProxyStreamHandler` and do not perform a local DNS query. Device routes resolve
and connect through the selected netstack table. Bulk TCP payload remains on a
kernel socket pair; networkd only owns control and recovery endpoints.

Resolver positive, negative, and pending state is keyed by networkd instance,
table, provider, normalized host, and record type. A and AAAA requests validate
DNS ID, response/question name and type, truncation, response code, bounded
CNAME traversal, record length, TTL, and RFC 2308 SOA negative TTL. Results are
limited to eight addresses and checked against the selected provider's allowed
IP routes. Retries stay within that provider's upstream list.

UDP/53, length-prefixed DNS-over-TLS, and HTTP POST DNS-over-HTTPS are live
transports. DoT/DoH connect directly to explicit bootstrap IPs, use configured
SNI and trustd roots, bound request/response sizes and deadlines, and never
bootstrap recursively through another resolver. DNS cache, pending request,
deadline, retry index, encoded idempotent request, and provider generation are
included in networkd heart-transplant state.

## Deliberate exclusions

This change provides the registration and delegation surfaces but does not add
SOCKS5, HTTP CONNECT, WireGuard, or IPsec engines. Redirectord, `go/` aliases,
encrypted alias storage, HTTP redirects, and synchronization remain future RFC
0061 work. Runtime registrations are ephemeral across reboot unless represented
in platform prototxt.

## Sources and validation

Implementation: [net.fidl](../../../idl/bexos/net/net.fidl),
[networkd routing](../../../services/networkd/src/routing.rs),
[networkd DNS](../../../services/networkd/src/dns.rs),
[networkd service](../../../services/networkd/src/service.rs), and
[networkd migration](../../../services/networkd/src/migration.rs).

Focused host coverage exercises suffix boundaries and ties, CIDR selection,
provider pinning/removal, DNS cache scoping, forged/mismatched replies, A/AAAA,
CNAME and negative TTL parsing, secure framing, migration records, proxy target
selection, and VRF enforcement. Architecture-specific guest status is recorded
separately in [testing status](../../testing-status.md).
