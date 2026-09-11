# RFC 0027: Userspace network architecture

- Created: 2026-08-29T21:23:42-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

A D1 Ethernet driver exchanges frame buffers with a userspace network stack. DNS and socket services are separated from in-process TLS, QUIC, and HTTP libraries.

## Design overview

In BexOS, the network architecture splits into two distinct layers: a **Hardware Device Driver (D1)** running `virtio-drivers` that exports raw Ethernet frame rings over shared memory, and a **Network Stack Service (`netstackd`)** running `smoltcp` in userspace that manages IP routing, TCP/UDP sockets, and exports standard POSIX-like and streaming socket FIDL protocols.

The current QEMU milestone implements the first userspace netstack substrate:
`d1_virtio_net` binds to `virtio-net-pci`, initializes the NIC through
`rcore-os/virtio-drivers`, and exposes a raw
`bexos.hardware.ethernet.Device` channel/FIFO-style surface. `netstackd` is
packaged as `bexos.service.netstackd`, preinstalled on the `STORAGE` partition,
launched by appd after disk drivers bind, and exposes the public
singleton `bexos.net.Netstack`. The hardware driver now deploys as a
std-linked Tokio D1 service and participates in userspace heart transplant by
preserving its Ethernet endpoints, FIFOs, shared VMOs, DMA pins, MMIO/ECAM
mappings, MAC/health/running state, and already-configured VirtIO transport and
queues without resetting the live NIC.

The implemented netstack milestone adds a virtio FIFO packet pump, shared
RX/TX VMO slot management, kernel `SOCKET` stream handles, component-configured
static IPv4, DHCP ACK lease adoption from received packets, UDP datagram
send/receive through smoltcp, static IPv6, IPv6 DNS/bootstrap config, link-local
IPv6 installation with SLAAC enablement, typed DNS A/AAAA caching with resolver
policy metadata, and a FIFO-backed smoltcp TCP runtime for connect/listen and
stream pumping. A field-replaceable `bexos.lib.net` SDK library now owns the
shared TLS/HTTP secure transport implementation used by distribution and timed,
while `netstackd` remains the transport and resolver service. TCP/listener
state is restored over retained Ethernet resources during heart transplant, and
established sockets carry a version-pinned smoltcp checkpoint for tuple,
sequence/window, timer, RTT/retransmit, assembler-range, option, and bounded
RX/TX byte state so replacement aborts instead of reconnecting a flow it cannot
restore.

## Layer 1: Hardware Driver (`d1_virtio_net`) & Ethernet FIFO FIDL

The VirtIO-Net driver wraps `virtio-drivers::net::VirtIONet` and provides a zero-copy shared memory interface to `netstack`. Rather than passing raw packets over IPC messages, packets are exchanged using a shared VMO ring buffer (FIFO) to achieve wire-speed throughput.
During heart transplant it snapshots and adopts direct VirtIO queue state and
HAL-owned DMA/MMIO resources, quiescing packet completions before activation so
replacement does not clear PCI device status or reinitialize queue registers.

### FIDL Definition (`bexos.hardware.ethernet`)

```fidl
library bexos.hardware.ethernet;

using bexos.kernel;

type MacAddress = struct {
    octets array<uint8, 6>;
};

type DeviceFeatures = strict bits : uint32 {
    PROMISCUOUS      = 0x0001;
    CHECKSUM_OFFLOAD = 0x0002;
    DMA_64BIT        = 0x0004;
};

type FrameEntry = struct {
    offset uint32; // Offset into the shared VMO
    length uint16; // Frame length in bytes
    flags uint16;  // e.g. TX complete, checksum status
};

@discoverable
protocol Device {
    /// Retrieve device MAC address and hardware capabilities
    GetInfo() -> (struct {
        status bexos.kernel.Status,
        mac MacAddress,
        mtu uint32,
        features DeviceFeatures
    });

    /// Set up zero-copy packet exchange buffers
    SetIoBuffers(resource struct {
        rx_vmo handle:VMO,
        tx_vmo handle:VMO,
        rx_fifo handle:FIFO, // Circular queue of FrameEntry structs
        tx_fifo handle:FIFO  // Circular queue of FrameEntry structs
    }) -> (struct {
        status bexos.kernel.Status
    });

    /// Start/Stop packet processing
    SetPromiscuous(struct { enabled bool }) -> (struct { status bexos.kernel.Status });
    Start() -> (struct { status bexos.kernel.Status });
    Stop() -> (struct { status bexos.kernel.Status });
};

```

## Layer 2: Network Service (`netstack` via `smoltcp`)

`netstack` connects to `bexos.hardware.ethernet.Device`, implements `smoltcp::phy::Device` on top of the shared VMO/FIFO queues, and exposes higher-level transport protocols to applications.

### FIDL Definition (`bexos.net`)

```fidl
library bexos.net;

using bexos.kernel;

struct Ipv4Address {
    octets array<uint8, 4>;
};

struct Ipv6Address {
    octets array<uint8, 16>;
};

type IpAddress = strict union {
    1: ipv4 Ipv4Address;
    2: ipv6 Ipv6Address;
};

type SocketAddress = struct {
    addr IpAddress;
    port uint16;
};

type SocketOptions = table {
    1: non_blocking bool;
    2: keep_alive_ms uint32;
    3: rx_buffer_size uint32;
    4: tx_buffer_size uint32;
};

/// Low-level TCP Stream Control Channel
protocol TcpSocket {
    /// Read/Write streams can be handed off to kernel STREAM handles for zero-IPC read/write
    GetStream() -> (resource struct {
        status bexos.kernel.Status,
        socket handle:SOCKET // Bi-directional kernel socket/stream primitive
    });

    GetPeerAddress() -> (struct { status bexos.kernel.Status, addr SocketAddress });
    GetLocalAddress() -> (struct { status bexos.kernel.Status, addr SocketAddress });
    Shutdown(struct { read bool, write bool }) -> (struct { status bexos.kernel.Status });
    Close();
};

/// TCP Listener
protocol TcpListener {
    Accept() -> (resource struct {
        status bexos.kernel.Status,
        client server_end:TcpSocket,
        peer_addr SocketAddress
    });
    Close();
};

/// UDP Datagram Channel
protocol UdpSocket {
    SendTo(struct {
        data vector<uint8>:8192,
        destination SocketAddress
    }) -> (struct { status bexos.kernel.Status, actual uint64 });

    RecvFrom() -> (struct {
        status bexos.kernel.Status,
        data vector<uint8>:8192,
        source SocketAddress
    });

    Bind(struct { local_addr SocketAddress }) -> (struct { status bexos.kernel.Status });
    Close();
};

/// High-level entry point exposed by netstack
@discoverable
protocol Netstack {
    ConnectTcp(resource struct {
        remote_addr SocketAddress,
        options SocketOptions,
        socket server_end:TcpSocket
    }) -> (struct { status bexos.kernel.Status });

    ListenTcp(resource struct {
        local_addr SocketAddress,
        options SocketOptions,
        listener server_end:TcpListener
    }) -> (struct { status bexos.kernel.Status });

    CreateUdpSocket(resource struct {
        options SocketOptions,
        socket server_end:UdpSocket
    }) -> (struct { status bexos.kernel.Status });

    /// DNS Resolution handled in netstack using smoltcp's DNS engine
    ResolveHost(struct {
        hostname string:255
    }) -> (struct {
        status bexos.kernel.Status,
        addresses vector<IpAddress>:8
    });
};

```

## Data Path Flow & Integration

```
[ WASM Application / POSIX libc / ConnectRPC ]
       │
       │ Calls `Netstack.ConnectTcp(addr)` -> returns `handle:SOCKET`
       ▼
[ netstack (smoltcp userspace daemon) ]
       │
       │ Polling loop runs smoltcp `Interface::poll()`
       │ Reads/writes packets to RX/TX FIFO descriptors
       ▼
[ Shared VMO Memory Pool (Pinned DMA Buffers) ]
       ▲
       │ VirtQueue descriptors mapped directly into VMO
       ▼
[ d1_virtio_net Driver (`virtio-drivers`) ]
       │
       │ Issues MMIO/PCIe reads/writes to QEMU virtio-net device
       ▼
[ QEMU / Hardware NIC ]

```

* **Zero Memory Copies on I/O:** `smoltcp` serializes IP packets directly into the pinned VMO ring shared with `virtio-drivers`. The driver adds the buffer descriptor into the VirtQueue without intermediary allocations.
* **Kernel Socket Fast-Path:** For TCP streams, `netstack` can link kernel `SOCKET` handles directly to socket buffers, allowing client applications to perform `read()` and `write()` calls with sub-microsecond latency.

DNS, TLS/QUIC, HTTP, and root certificates should be divided between the **network service (`netstack`)** and **in-process client libraries** based on whether the responsibility involves shared system state or bulk cryptographic compute.

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ APPLICATION PROCESS (WASM Container / Native Client)                        │
│                                                                             │
│  [ In-Process Libraries ]                                                   │
│  • `bexos.lib.net` HTTP/1.1, HTTP/2, HTTP/3 SDK surface                    │
│  • `bexos.lib.net` TLS 1.3 / QUIC Engine ABI                               │
│  • Certificate Path Validation Engine (webpki)                              │
│                                                                             │
│  Data Path: Decrypted plain text is processed directly in-memory.           │
│  Encrypted TLS/QUIC frames pass directly over kernel `SOCKET` handles.      │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ System Capabilities & Shared State
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ SYSTEM DAEMONS (Shared State & Security Policy)                             │
│                                                                             │
│  [ netstack ]                                                               │
│  • System DNS Resolver Daemon (Caching, DoH policy, Split-Horizon VPN, MDNS)│
│  • TCP / UDP / IP Routing & Socket Lifecycle                                │
│                                                                             │
│  [ trustd / ca-certificates ]                                               │
│  • System & Enterprise Root CA Store (`/system/certs/cacerts.redb`)         │
│  • Read-only VMO export of validated Trust Anchors                          │
└─────────────────────────────────────────────────────────────────────────────┘

```

## Layer-by-Layer Architecture

### DNS: Inside the Network Service (`netstack`)

DNS belongs **centrally in `netstack`**, exposed via FIDL (`bexos.net.Resolver`):

* **System-Wide Caching:** Avoids duplicate queries across dozens of concurrently running apps.
* **Encrypted DNS (DoH / DoT):** `netstack` can transparently enforce DNS-over-HTTPS or DNS-over-TLS system-wide without relying on individual apps to implement it.
* **VPN & Split-Horizon Routing:** When an enterprise VPN or private cloud connection opens, `netstack` dynamically routes domain queries (e.g., `*.corp.internal`) to the VPN tunnel while sending public domains to the default resolver.
* **mDNS / Local Link Discovery:** Centralized discovery prevents port `5353` binding conflicts.

### TLS 1.3 & QUIC: In-Process Client Libraries (e.g., `rustls`, `quiche`)

TLS encryption and QUIC packet framing belong **inside the application process**:

* **Zero-Copy Crypto:** Moving encrypted TLS/QUIC handshakes and stream multiplexing into a central IPC service would require copying raw decrypted payload bytes across process boundaries, halving memory bandwidth.
* **Process Isolation:** The app's session keys and decrypted memory buffers never leave the app sandbox.
* **Direct Socket Fast-Path:** The client library establishes a raw UDP socket (for QUIC) or TCP socket (for TLS) via `netstack` and pumps encrypted bytes directly through kernel stream handles.

### HTTP / HTTP/3: In-Process Client Libraries

HTTP parsing (headers, framing, HPACK/QPACK compression, connection pooling) belongs **strictly inside the application process**:

* HTTP implementations evolve rapidly and are tightly coupled to framework language ecosystems (e.g., Tokio/Hyper in Rust, fetch in JS/QuickJS, standard Net/HTTP in Go).
* Keeping HTTP in user libraries allows apps to use customized connection pooling, HTTP/3 streaming, or lightweight embedded parsers without requiring OS updates.

### Root Certificates: Central System Store with In-Process Verification

Trust anchors must be **centrally managed by the OS**, while verification runs **locally in the app**:

* **Storage (`trustd` / `/system/certs/cacerts.redb`):** The system manages the root CA trust bundle. Enterprise MDMs or users can add corporate roots or revoke untrusted certificates in one place.
* **Read-Only Shared Memory Mount:** The root bundle is exposed to app sandboxes as an immutable, read-only VMO or `/pkg/certs/ca-bundle.bin` mount.
* **In-Process Verification:** The client-side TLS library (`rustls` + `webpki`) reads the shared root anchors and validates the server certificate chain in-process during the handshake.

## Summary Matrix

| Component | Placement | Reason |
| --- | --- | --- |
| **DNS Resolver** | **Service (`netstack`)** | Central caching, DoH/DoT policy, VPN split-routing, mDNS. |
| **Root CA Store** | **Service / Central File** | Unified updates, enterprise MDM cert injection, revocation lists. |
| **TLS 1.3 Engine** | **In-Process Library** | Eliminates IPC copying; keeps private keys in the caller sandbox. |
| **QUIC Protocol** | **In-Process Library** | Direct UDP socket pumping; zero-copy stream framing. |
| **HTTP/1/2/3** | **In-Process Library** | Protocol flexibility, framework integration, custom multiplexing. |
