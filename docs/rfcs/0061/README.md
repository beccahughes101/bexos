# RFC 0061: Unified egress and split-DNS routing

- Created: 2026-09-10T15:55:54-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

A unified outbound provider model routes traffic through IP-layer VPNs and application proxies. Domain-aware connection requests enable split DNS, and redirectord resolves local short links.

## Design overview

Treating IP-layer VPNs and application-layer proxies through a unified outbound routing engine eliminates the friction between L3 interfaces (TUN/WireGuard) and L4/L7 proxies (SOCKS5, HTTP CONNECT).

By making the transport connection primitives accept high-level destinations (`Domain` or `IpAddress`) rather than forcing caller-side resolution, `networkd` can evaluate both IP routing tables and domain-suffix routing rules before dispatching traffic.

## Unified Egress Model: `NetworkProvider`

Instead of bifurcating the stack into "virtual network adapters" and "proxy daemons", `networkd` treats every outbound route target as an implementer of a unified provider protocol:

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ APPLICATION RUNTIME (Rust / WASM / MicroVM)                                 │
│ Calls: `networkd.ConnectTcp(target: "jira.internal.corp:443")`              │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ FIDL Channel
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ `networkd` ROUTING & RESOLUTION CORE                                        │
│                                                                             │
│ 1. Match Suffix Rules: "*.corp" ──► WireGuard VPN Provider                  │
│ 2. Match Domain Rules: "go/*"   ──► Local Redirector Service                │
│ 3. Match Suffix Rules: "*.onion"──► SOCKS5 / Tor Provider                   │
│ 4. Fallback Default: Resolve via public DNS ──► Direct Default Gateway      │
└──────────┬───────────────────────────┬───────────────────────────┬──────────┘
           │ IP Route Hand-off         │ Direct SOCKS Stream       │ Rewrite
           ▼                           ▼                           ▼
┌───────────────────────┐   ┌───────────────────────┐   ┌─────────────────────┐
│ WireGuard Egress (L3) │   │ SOCKS5 Egress (L4/L7) │   │ `redirectord`       │
│ • Packet Ring / TUN   │   │ • Submits domain name │   │ • Rewrites `go/foo` │
│ • Bound to corp DNS   │   │   directly to proxy   │   │   to HTTP 302 / IPC │
└───────────────────────┘   └───────────────────────┘   └─────────────────────┘

```

* **L3 IP VPNs (WireGuard, IPsec):** Register a virtual packet-swapping interface (`TunDevice`) and install IP CIDRs and DNS search domains into `networkd`.
* **L4/L7 Proxies (SOCKS5, HTTP CONNECT):** Register as connection-stream delegates. When a domain matches their assigned pattern, `networkd` avoids local DNS resolution entirely and delegates the raw domain string to the proxy handshake, preserving privacy and avoiding DNS poisoning.

## Domain-Aware Sockets & FIDL Definitions

Modifying the socket factory to take an untyped target endpoint allows the network stack to intercept connection requests before name resolution:

```fidl
library bexos.net;

using bexos.kernel;

type DomainName = string:255;
type Port = uint16;

type IpAddress = flexible union {
    1: ipv4 array<uint8, 4>;
    2: ipv6 array<uint8, 16>;
};

type Endpoint = flexible union {
    1: ip struct {
        addr IpAddress;
        port Port;
    };
    2: domain struct {
        host DomainName;
        port Port;
    };
};

@discoverable
protocol SocketProvider {
    /// Opens a stream socket. Can take IP:Port or Domain:Port.
    ConnectTcp(resource struct {
        target Endpoint;
    }) -> (resource struct {
        socket zx.Handle:SOCKET;
    }) error bexos.kernel.Status;
};

```

## Provider Registration & Split-DNS Routing Table

Providers register their capabilities with `networkd` using dynamic route declarations that include both IP subnets and DNS domain namespaces:

```fidl
library bexos.net.manager;

using bexos.net;
using bexos.kernel;

type RouteTarget = flexible union {
    /// Physical or virtual packet device (WireGuard, Ethernet)
    1: device_id uint64;
    /// Stream proxy channel (SOCKS5 daemon, HTTP CONNECT tunnel)
    2: proxy_channel client_end:ProxyStreamHandler;
};

struct DnsUpstream {
    ip bexos.net.IpAddress;
    port uint16;
    /// Optional TLS/DoH server name for encrypted upstream resolution
    tls_server_name string:128;
};

struct ProviderRouteConfig {
    /// CIDR blocks handled by this provider (e.g., "10.0.0.0/8")
    ip_routes vector<string:48>:16;

    /// Domain suffixes routed exclusively to this provider (e.g., ["corp.internal", "dev"])
    domain_routes vector<string:128>:16;

    /// DNS servers associated with this provider
    dns_servers vector<DnsUpstream>:4;

    /// Route priority / metric (lower = higher preference)
    priority uint32;
};

@discoverable
protocol NetworkRoutingManager {
    RegisterEgressProvider(resource struct {
        name string:32;
        target RouteTarget;
        config ProviderRouteConfig;
    }) -> (struct {
        provider_token uint64;
    }) error bexos.kernel.Status;

    UnregisterEgressProvider(struct {
        provider_token uint64;
    }) -> () error bexos.kernel.Status;
};

```

## Resolution Pipeline: Split-DNS Routing

When `ConnectTcp(Endpoint::Domain { host: "db.corp.internal", port: 5432 })` arrives:

1. **Exact Suffix Match:** `networkd` inspects the domain name against registered `domain_routes`.
2. **Path A (Target is a Proxy):** If the match is a SOCKS5 provider, `networkd` skips name resolution. It establishes a TCP session to the proxy and transmits the unhashed hostname inside the SOCKS5 `ATYP 0x03` header.

### Path B (Target is a TUN/WireGuard Egress)

* The query is directed specifically to the `dns_servers` attached to that provider (preventing split-DNS leakage to the public network).
* The resolved IP is matched against the provider's `ip_routes` to guarantee packets exit over the tunnel device.

4. **Path C (No Match):** The request falls back to platform default DNS resolvers and uses the default internet routing gateway.

## The Local Redirector Service (`redirectord`) for Short Links

To support corporate or personalized short-links (such as `go/links` or `gh/issue/42`) without modifying global DNS records:

* **Domain Ownership:** `redirectord` registers the pseudo-TLD `go` (or individual single-word prefixes) as a `domain_route` with `networkd`.

### Fast Local Resolution

When a browser or tool queries `http://go/standup`:
1. `networkd` routes requests for `*.go` to `redirectord`.
2. For DNS-based lookups, `redirectord` responds with a local loopback IP (e.g., `127.0.0.1` or a dedicated virtual IP).
3. An internal lightweight HTTP redirect daemon listening on that IP looks up the key in an encrypted database (backed by `prefsd` or synchronized via an enterprise sync service) and returns an instant `HTTP 302 Found` to the destination URL.

* **App-Direct Protocol:** Native BexOS apps can talk directly to `redirectord` via FIDL to resolve tokens (`ResolveAlias("standup") -> "https://meet.google.com/xyz"`), skipping HTTP round-trips altogether.
