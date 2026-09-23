# RFC-0068: Multi-Instance Network Stacks, Virtual Routing Domains, and Capability-Routed Networking

* **Author:** BexOS Networking & Security Working Group
* **Status:** Proposed
* **Target Subsystems:** `networkd`, `driver_manager`, `appd`, `vswitchd`, `devhost`
* **Applicability:** Cloud Routers, Multi-Homed Edge Gateways, Enterprise VPN Sandboxes, MicroVMs

---

## 1. Summary

This RFC specifies the multi-tenant and multi-stack networking architecture for BexOS. It introduces:

1. **Multi-Instance Network Stacks (`netstack-*`):** The ability to instantiate multiple, completely independent D1 network stack processes in separate address spaces and resource jobs.
2. **Capability-Injected Stack Routing:** Zero-ambient networking where client applications bind to specific network stacks (`system_default`, `corp_vpn`, `guest_dmz`) strictly via handle routing in their private incoming namespaces (`/svc/bexos.net.SocketProvider`) configured by `appd`.
3. **Internal Virtual Routing & Forwarding (VRF) Engine:** In-process multi-table routing within a single `netstack` instance for high-density, low-memory routing appliance deployments.
4. **Packet Multiplexing via `vswitchd`:** A lightweight zero-copy software switch multiplexing a single physical NIC across multiple isolated `netstack` instances via shared memory (`VMO`) rings.

---

## 2. Motivation

Traditional monolithic network stacks (such as the Linux network namespace or Windows network stack) present architectural hazards when applied to capability-based microkernels:

* **Shared In-Memory State & Exhaustion:** In traditional OSs, running a VPN or separate routing table inside the same kernel or process means socket memory exhaustion, TCP state-table saturation (e.g., SYN floods), or ARP cache poisoning in one domain impacts the entire system.
* **Ambient Stack Selection & VPN Leakage:** Applications traditionally invoke ambient syscalls or manipulate socket options (e.g., `SO_BINDTODEVICE`, `SO_MARK`) to bind to interfaces. If a VPN daemon crashes or drops routes, traffic silently leaks over the default interface because the application still holds ambient raw internet capabilities.
* **Driver & Stack Coupling:** Forcing all physical NICs and virtual tunnels through a single monolithic `netstack` limits hardware fault recovery and prevents per-app zero-trust network policy enforcement.

BexOS resolves these problems by treating the network stack as an unprivileged, composable service that can be instantiated per security domain or run as a multi-table router.

---

## 3. Dual-Model Architecture: Isolation vs. Density

BexOS provides two operational models for network segmentation:

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ MODEL A: HARD PROCESS ISOLATION (Security / Zero-Trust / Enterprise)        │
│                                                                             │
│  [ Untrusted App ]                  [ System Apps / General Browsing ]      │
│         │                                          │                        │
│         ▼ `/svc/bexos.net.SocketProvider`          ▼                        │
│  ┌──────────────┐                          ┌──────────────┐                 │
│  │ `netstack-1` │ (Address Space 1)        │ `netstack-0` │ (Address Space 0│
│  │ (Corp VPN)   │                          │ (Default)    │                 │
│  └──────┬───────┘                          └──────┬───────┘                 │
│         │ VMO Packet Ring                         │ VMO Packet Ring         │
│         └────────────────────┬────────────────────┘                         │
│                              ▼                                              │
│             ┌──────────────────────────────────┐                            │
│             │ `vswitchd` / Physical NIC Rings  │                            │
│             └──────────────────────────────────┘                            │
└─────────────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────────────┐
│ MODEL B: IN-PROCESS VRF (Cloud Edge Router / High-Density Appliance)        │
│                                                                             │
│  Interface eth0 (VLAN 10) ──► [ FIB Table 1 ] ──► Socket Token (Table 1)    │
│  Interface eth1 (VLAN 20) ──► [ FIB Table 2 ] ──► Socket Token (Table 2)    │
│                                                                             │
│  * Single `netstack` process; multiple routing tables indexed by `TableId`. │
│  * Ultra-low memory footprint (< 100 KB per additional routing domain).     │
└─────────────────────────────────────────────────────────────────────────────┘

```

| Dimension | Model A: Multi-Instance Process Isolation | Model B: In-Process VRF Routing |
| --- | --- | --- |
| **Fault Isolation** | **Total:** Stack panic, memory leak, or exploit isolates to one process. | **Process-level:** A bug in routing logic crashes all domains. |
| **Memory Cost** | ~4–12 MB base per `netstack` instance. | Negligible (~50–200 KB per VRF table). |
| **DNS / State Leaks** | **Impossible:** No channel exists to alternative stack or DNS daemon. | **Policy-enforced:** Stack must enforce table isolation internally. |
| **Best Used For** | Work/Personal device profiles, MicroVMs, Enterprise VPNs, Tor/Anon sandboxes. | Edge routers, multi-tenant cloud appliances, VLAN aggregation switches. |

---

## 4. Multi-Instance Isolated Topology (Model A)

### 4.1 Orchestration without Ambient Process Spawning

`netstack` instances do **not** spawn child stacks. Instead, the platform service coordinator (`appd` or `networkd`) provisions separate instances under distinct capability jobs.

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ `appd` (Application Lifecycle & Capability Manager)                         │
│                                                                             │
│  1. Reads application manifest: `network_domain: "corp_vpn"`                │
│  2. Resolves `corp_vpn` to active or lazy `netstack-corp` instance         │
│  3. Routes channel handle directly into `/svc/bexos.net.SocketProvider`     │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ Launches sandbox with injected handle
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ APPLICATION SANDBOX                                                         │
│                                                                             │
│  • Performs standard `TcpStream::connect("internal.server:443")`            │
│  • Completely unaware that traffic routes exclusively via `netstack-corp`   │
│  • Impossible to leak packets to default gateway (no handle exists)         │
└─────────────────────────────────────────────────────────────────────────────┘

```

### 4.2 Shared Physical NIC Multiplexing via `vswitchd`

When multiple independent `netstack` instances share a single physical Ethernet adapter, a high-throughput, unprivileged multiplexer (`vswitchd`) acts as the L2 dispatch layer:

1. **Hardware Ingestion:** `devhost` passes its `NetworkDeviceDataPlane` packet ring handle to `vswitchd`.
2. **Virtual Port Allocation:** `vswitchd` provisions isolated shared-memory VMO descriptor rings for each registered `netstack` instance (`vport0`, `vport1`).
3. **L2 Frame Demuxing:**
* Inbound packets are inspected for destination MAC address, VLAN ID (IEEE 802.1Q), or encapsulation headers.
* Frames matching a stack's filter are copied or page-flipped into that stack's RX ring.
* Outbound packets from any stack are multiplexed directly into the physical TX ring using lock-free ring buffers.



---

## 5. In-Process VRF Architecture (Model B)

For high-density edge appliance configurations, a single `netstack` process maintains multiple isolated Forwarding Information Bases (FIBs):

```rust
pub struct NetstackRouter {
    tables: HashMap<TableId, ForwardingTable>,
    interfaces: HashMap<InterfaceId, TableId>,
}

#[derive(Copy, Clone, Hash, PartialEq, Eq)]
pub struct TableId(pub u32);

```

### 5.1 Socket Token Attenuation

When an application or microVM connects to `bexos.net.SocketProvider`, the caller presents an attenuated token representing its authorized routing table:

```
App Connect ──► SocketProvider.CreateSocket(table_token, Protocol::TCP)
                     │
                     ▼
           Verify Token Rights
                     │
                     ├─► TableId(0): Internet FIB
                     └─► TableId(100): Private Backhaul FIB

```

Sockets created within `TableId(100)` can only resolve routes, bind to interfaces, and accept connections associated with that table ID. All ARP/NDP neighbor caches, routing tables, and interface bindings are strictly scoped to the table identifier.

---

## 6. Interface Definition Language (FIDL)

The multi-stack protocols are defined in `idl/bexos/net/stack.fidl`:

```fidl
library bexos.net;

using bexos.kernel;
using bexos.hardware;

type TableId = uint32;

type IpAddress = flexible union {
    1: ipv4 array<uint8, 4>;
    2: ipv6 array<uint8, 16>;
};

struct SocketAddress {
    addr IpAddress;
    port uint16;
};

@discoverable
protocol SocketProvider {
    /// Request an asynchronous stream socket (TCP)
    CreateStreamSocket(struct {
        domain uint32; // AF_INET, AF_INET6
    }) -> (resource struct {
        socket_channel zx.Handle:CHANNEL;
    }) error bexos.kernel.Status;

    /// Request a datagram socket (UDP)
    CreateDatagramSocket(struct {
        domain uint32;
    }) -> (resource struct {
        socket_channel zx.Handle:CHANNEL;
    }) error bexos.kernel.Status;
};

@discoverable
protocol StackController {
    /// Attach a physical or virtual network device packet ring to this stack
    AttachInterface(resource struct {
        interface_name string:32;
        device_dataplane bexos.hardware.NetworkDeviceDataPlane;
        assigned_table TableId;
    }) -> () error bexos.kernel.Status;

    /// Configure routing entry inside a specific FIB table
    AddRoute(struct {
        table TableId;
        destination IpAddress;
        prefix_len uint8;
        gateway box<IpAddress>;
        interface_name string:32;
    }) -> () error bexos.kernel.Status;

    /// Provision an attenuated SocketProvider bound strictly to one TableId
    GetScopedSocketProvider(struct {
        table TableId;
    }) -> (resource struct {
        provider_channel zx.Handle:CHANNEL;
    }) error bexos.kernel.Status;
};

```

---

## 7. Lifecycle, Fault Isolation & Heart Transplant

Because each `netstack` in Model A is an isolated component:

1. **Crash Isolation:** If `netstack-corp` panics due to an invalid TCP packet or state bug, `netstack-default` continues routing traffic with zero packet drops.
2. **State Stashing & Recovery:**
* `vswitchd` stashes the client channel endpoints when a stack dies.
* `networkd` instantiates a fresh `netstack-corp` process.
* Active TCP connection descriptors stored in a shared backup VMO are re-mapped into the new process.
* Traffic resumes without tearing down user applications.


3. **Fate-Sharing for Ephemeral Sandboxes:** If a dedicated network stack is spun up exclusively for an isolated microVM or container, the stack is placed inside the VM's parent job. When the VM exits, the kernel tears down the network stack simultaneously, freeing all memory.

---

## 8. Security & Threat Matrix

| Threat Scenario | System Defense / Mitigation |
| --- | --- |
| **Split-Tunnel Leakage (VPN failure)** | Sandboxed enterprise apps only hold a channel handle to `netstack-corp`. There is no ambient system socket syscall; if the VPN stack drops, traffic halts completely rather than falling back to the public internet. |
| **DNS Spoofing & Cache Poisoning** | Each stack maintains its own isolated resolver cache or binds to distinct upstream DoH resolvers. A compromised guest network cannot poison records for enterprise or system services. |
| **TCP SYN Flood / Resource Starvation** | Stacks run in distinct kernel Jobs with independent CPU deadlines and memory limits. A DoS attack targeting a public-facing interface exhausts only the public stack's budget, leaving the management/internal stack unaffected. |
| **Raw Packet Sniffing** | Applications do not have raw packet capture privileges. Device packet rings are held strictly by `devhost`, `vswitchd`, or the designated `netstack`. |

---

## 9. Implementation Roadmap

### Phase 1: In-Process VRF Support

* Extend the core Rust `netstack` implementation to support multiple `ForwardingTable` instances indexed by `TableId`.
* Implement `GetScopedSocketProvider` FIDL interface to allow minting attenuated socket providers.

### Phase 2: Multi-Process Stack Instantiation

* Update `networkd` to orchestrate multiple named `netstack` components (`netstack-default`, `netstack-vpn`).
* Update `appd` to inspect component manifests (`network_domain`) and route the matching `SocketProvider` capability into the application's namespace.

### Phase 3: Software Switch (`vswitchd`)

* Implement the lightweight `vswitchd` multiplexer utilizing lock-free VMO shared-memory descriptor rings.
* Connect physical NIC drivers from `driver_manager` through `vswitchd` to distribute traffic across concurrent netstack processes.