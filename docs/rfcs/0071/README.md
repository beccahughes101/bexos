# RFC-0071: Vector Packet Processing (VPP) Graph Architecture in `vswitchd` and In-Process Sandboxed WebAssembly Network Extensions

* **Author:** BexOS Networking, High-Performance Systems & Security Working Group
* **Status:** Proposed
* **Target Subsystems:** `vswitchd`, `netstack`, `driver_manager`, `pkgd`, `sdk/fidl/bexos.net`
* **Applicability:** Fast-Path Switching, Stateful Firewalls, NAT, MicroVM/Container Egress, Edge Routers

> **Implementation note (2026-09-24):** See [CURRENT.md](CURRENT.md). Native
> JIT, direct VMO/zero-copy guest access, NAT64, line-rate operation, and the
> sub-nanosecond figures below remain future or unverified design targets. The
> current implementation uses Pulley and bounded vector copies and makes no
> throughput claim.

---

## 1. Summary

This RFC specifies the high-performance data plane architecture for BexOS. It introduces:

1. **The `vswitchd` Vector Packet Processing (VPP) Graph:** Refactoring the `vswitchd` L2/L3 software switch from a scalar frame processor into a batch-oriented, directed acyclic graph (DAG) pipeline inspired by FD.io VPP. Packets travel in vectors of up to 256 frame descriptors through cache-hot graph nodes.
2. **In-Process WebAssembly (WASM) Extension Engine:** Safe, JIT-compiled dynamic execution of network functions (stateful firewalls, packet inspection, NAT, flow routing) directly inside `vswitchd` using an embedded Wasmtime runtime.
3. **Vectorized WASM Foreign Function Interface (FFI):** An ABI design that amortizes WASM boundary-crossing overhead across vectors of 64–256 frames, reducing runtime switching costs to sub-nanosecond amortized execution per packet.
4. **Dynamic Plugin Delivery via `pkgd`:** Packaging and distribution of network extensions as signed, content-addressed OCI artifacts (`bexos.net.filter.v1`) hot-reloaded into the running graph with zero downtime.

---

## 2. Motivation

In high-throughput microkernel networking (10GbE to 100GbE line rates), conventional designs suffer from two primary bottlenecks:

* **The Scalar Processing Bottleneck (Instruction Cache Thrashing):** Traditional packet processors loop over packets one at a time (`packet -> parse_eth -> lookup_acl -> nat -> forward`). For each packet, the CPU swaps executable code out of L1 instruction cache, incurring constant pipeline stalls.
* **The IPC and Architectural Boundary Tax:** Moving packets over IPC channels to user-space daemons (`netstack`) just to drop or NAT them wastes memory bandwidth, context-switch budgets, and CPU cache lines. Furthermore, invoking sandboxed plug-in logic (such as eBPF or WebAssembly) on a per-packet basis incurs boundary-crossing overheads (5–15 ns per invocation), which alone exceeds the entire per-packet time budget at 100GbE (~6.72 ns).

BexOS adopts a vector graph engine inside `vswitchd` and extends it with vectorized WebAssembly execution, ensuring that packets are filtered, NAT'd, and routed at line rate before reaching higher-level consumers.

---

## 3. System Architecture & Placement: `vswitchd` vs. `netstack`

BexOS strictly divides network responsibilities across the data plane and transport termination plane:

* **`vswitchd` (L2–L4 Fast Forwarding Plane):** Owns physical NIC packet rings (`devhost`), virtual interface rings (`vport`), vector graph routing, stateless/stateful ACLs, NAT, and encapsulation. All in-process WASM network extensions live here.
* **`netstack` (L4–L7 Transport Termination Plane):** Consumes filtered packets from `vswitchd` via shared `VMO` rings. Responsible strictly for TCP/QUIC state machines, congestion control, TLS, and application socket endpoints (`bexos.net.SocketProvider`).

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ PHYSICAL HARDWARE / DRIVERS                                                 │
│ `devhost-eth0` (Direct Hardware DMA via Memory-Mapped VMO Ring)             │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ Raw Descriptors (Vector Batch: 256)
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ `vswitchd` (D1 Vector Packet Processing Daemon)                             │
│                                                                             │
│  [ Node: hw-nic-rx ] ──► Vector Poll from Shared VMO Descriptor Ring       │
│           │                                                                 │
│           ▼ (Vector: 256 Buffers)                                           │
│  [ Node: ethernet-input ] (Validates L2 headers, strips VLAN 802.1Q)        │
│           │                                                                 │
│           ▼                                                                 │
│  ┌───────────────────────────────────────────────────────────────────────┐  │
│  │ DYNAMIC WASM GRAPH NODE (`wasmtime` In-Process JIT Sandbox)            │  │
│  │ Module: `firewall-stateful.wasm`                                      │  │
│  │ • Operates on entire Vector Batch in a single FFI call                │  │
│  │ • Zero memory copy: Directly references pinned VMO offsets             │  │
│  │ • Action: Marks frame indices for DROP, REWRITE (NAT), or FORWARD     │  │
│  └───────────────────────────────────────────────────────────────────────┘  │
│           │                                          │                      │
│           ▼ (Frames marked DROP)                     ▼ (Frames marked PASS) │
│  [ Node: packet-drop-counter ]              [ Node: ip4-lookup-nat ]        │
│                                                      │                      │
│                                                      ▼                      │
│                                             [ Node: demux-vport ]           │
└───────────────────┬─────────────────────────────────────┬───────────────────┘
                    │ VMO Ring A                          │ VMO Ring B
                    ▼                                     ▼
┌──────────────────────────────────────┐ ┌────────────────────────────────────┐
│ `netstack-default` (System TCP/UDP)  │ │ `starnix_runner` (Linux Container) │
└──────────────────────────────────────┘ └────────────────────────────────────┘

```

---

## 4. The VPP-Inspired Directed Graph Engine

### 4.1 Vector Processing Mechanics

Instead of scalar iteration, `vswitchd` defines an acyclic execution graph where each node processes a **Vector Batch** of up to 256 buffer descriptors (`VLIB_FRAME_SIZE = 256`).

```
Scalar (Traditional):
P1: [Eth] -> [Firewall] -> [NAT] -> [Route]
P2: [Eth] -> [Firewall] -> [NAT] -> [Route]  <-- Continual I-Cache evictions

Vector (BexOS VPP):
[P1..P256]: ──► [ Node: Ethernet Input ]      <-- Code stays warm in L1 I-Cache
                     │
                     ▼
[P1..P256]: ──► [ Node: WASM Firewall ]       <-- Single WASM transition for 256 pkts
                     │
                     ▼
[P1..P256]: ──► [ Node: IPv4 NAT ]

```

When a batch of packets traverses a graph node, the machine code for that specific node stays resident in the CPU's L1 Instruction Cache ($L1_i$), achieving sustained IPC throughput approaching theoretical memory bandwidth limits.

### 4.2 Core Graph Node Trait (Rust)

```rust
pub const VECTOR_BATCH_SIZE: usize = 256;

#[repr(C, align(64))]
pub struct PacketDescriptor {
    pub vmo_offset: u64,
    pub length: u32,
    pub port_id: u16,
    pub flags: u16,
    pub l2_offset: u8,
    pub l3_offset: u8,
    pub l4_offset: u8,
    pub flow_hash: u32,
}

pub struct VectorBatch {
    pub descriptors: [PacketDescriptor; VECTOR_BATCH_SIZE],
    pub count: u16,
    pub next_nodes: [u16; VECTOR_BATCH_SIZE],
}

pub trait GraphNode: Send + Sync {
    /// Name of this pipeline processing step
    fn name(&self) -> &'static str;

    /// Vector dispatch loop: processes all frames in `batch`
    fn process_vector(&mut self, ctx: &mut GraphContext, batch: &mut VectorBatch);
}

```

---

## 5. In-Process WebAssembly Extension Engine

BexOS rejects arbitrary kernel-level bytecode interpreters (such as Linux eBPF) due to security verification complexities and fixed instruction constraints. Instead, `vswitchd` embeds an optimized **Wasmtime** runtime.

### 5.1 Vectorized WASM ABI Contract

Calling a WebAssembly function incurs an execution context transition (~10 ns). Invoking a WASM filter per-packet destroys line-rate performance.

BexOS overcomes this by standardizing a **Vectorized Packet ABI**: the host passes an entire slice of 256 packet metadata descriptors and a memory-mapped buffer window into the WebAssembly instance in a **single call**.

#### Exported WASM Guest Symbol

```c
// ABI exposed by the guest WASM plugin (e.g., compiled from Rust/C/Zig)
__attribute__((export_name("bexos_filter_process_vector")))
uint32_t bexos_filter_process_vector(
    uint32_t batch_count,
    const WasmPacketDescriptor* descriptors,
    uint16_t* next_actions
);

```

#### Amortized Cost Calculation

$$\text{Transition Cost per Packet} = \frac{10\text{ ns (WASM context switch)}}{256\text{ packets in vector}} \approx 0.039\text{ ns/packet}$$

At 100GbE line rate (148.8 million packets per second), the total CPU time spent transitioning across the WASM boundary is reduced from **100% CPU saturation to less than 0.6% overhead**.

### 5.2 Zero-Copy Memory Windowing

1. **Shared Packet Pages:** Packets reside in physical memory pages pinned inside microkernel Virtual Memory Objects (`VMO`).
2. **Linear Memory Mapping:** `vswitchd` utilizes Wasmtime’s linear memory subsystem to map the active VMO read-only buffer window directly into the WASM module's virtual address space.
3. **Descriptor Passing:** The host passes array offsets and slice bounds. The WASM plugin reads and validates packet bytes directly out of mapped memory with **zero intermediate `memcpy` operations**.
4. **Action Output:** The WASM code writes a 16-bit action code into `next_actions[i]`:
* `0`: `DROP`
* `1`: `PASS` (Continue to default routing)
* `2`: `NAT_REWRITE` (Commit header rewrites back to descriptor)
* `3`: `REDIRECT_VPORT(id)`



---

## 6. Fault Isolation, Determinism & Dynamic Lifecycle

Network extensions operate under a zero-trust model:

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ WASM EXTENSION CONTAINMENT IN `vswitchd`                                    │
│                                                                             │
│ 1. Memory Safety:                                                           │
│    WASM memory bounds are strictly enforced by hardware page guard zones    │
│    and Cranelift compiler checks. Out-of-bounds reads trap immediately.     │
│                                                                             │
│ 2. Epoch-Based Execution Throttling (Infinite Loop Prevention):             │
│    `vswitchd` increments an execution epoch every 500 microseconds.         │
│    Wasmtime tracks engine fuel / epoch ticks. If a plugin fails to complete │
│    its vector batch within its allocation, the instance traps.              │
│                                                                             │
│ 3. Fail-Open / Fail-Closed Policy:                                          │
│    On trap or panic:                                                        │
│    • `vswitchd` catches the trap safely without crashing the main daemon.   │
│    • Telemetry increments error counters (`plugin.panics`).                 │
│    • Traffic falls back to the declared failure policy (Drop or Pass).      │
└─────────────────────────────────────────────────────────────────────────────┘

```

### 6.1 Dynamic Packaging & Ingestion via `pkgd`

WASM extensions are compiled into standalone OCI artifacts:

```json
{
  "schemaVersion": 2,
  "mediaType": "application/vnd.oci.image.manifest.v1+json",
  "artifactType": "application/vnd.bexos.net.filter.v1",
  "config": {
    "mediaType": "application/vnd.bexos.net.filter.config.v1+json",
    "digest": "sha256:5b8e9..."
  },
  "layers": [
    {
      "mediaType": "application/vnd.bexos.net.filter.wasm.v1",
      "digest": "sha256:d8c3e...",
      "size": 142800,
      "annotations": {
        "bexos.org/plugin-name": "stateful-firewall",
        "bexos.org/target-abi": "vpp-wasm-v1"
      }
    }
  ]
}

```

* **Hot Swapping:** `vswitchd` connects to `pkgd` via `libpkg_client`. When an updated firewall plugin is approved, `pkgd` verifies the TUF signature and supplies a read-only VMO.
* **Atomic Graph Mutation:** `vswitchd` pre-compiles the WASM bytecode using Cranelift in a background thread, spins up the new instance, and atomically updates the graph node's pointer. In-flight vectors complete using the previous instance, while subsequent vectors execute against the new module with **zero dropped frames**.

---

## 7. Interface Definition Language (FIDL)

The switch control and plugin registration APIs live in `idl/bexos/net/switch.fidl`:

```fidl
library bexos.net;

using bexos.kernel;

type FilterAction : uint16 {
    DROP = 0;
    PASS = 1;
    REDIRECT = 2;
};

type FailurePolicy : uint8 {
    FAIL_CLOSED = 0;
    FAIL_OPEN = 1;
};

@discoverable
protocol SwitchController {
    /// Loads a compiled WASM network extension into the active processing graph
    InstallExtension(resource struct {
        plugin_name string:64;
        wasm_binary zx.Handle:VMO;
        insertion_point string:64; // e.g. "after:ethernet-input"
        policy FailurePolicy;
    }) -> () error bexos.kernel.Status;

    /// Detaches and unloads a named extension from the processing graph
    RemoveExtension(struct {
        plugin_name string:64;
    }) -> () error bexos.kernel.Status;

    /// Provisions an isolated virtual port (vport) for a client (netstack or VM)
    CreateVirtualPort(struct {
        vport_name string:32;
        mtu uint32;
    }) -> (resource struct {
        rx_ring zx.Handle:VMO;
        tx_ring zx.Handle:VMO;
        doorbell zx.Handle:CHANNEL;
    }) error bexos.kernel.Status;
};

```

---

## 8. Performance Targets & Evaluation Metrics

| Metric | Target (Standard Hardware) | Comparison: Scalar Linux eBPF / iptables |
| --- | --- | --- |
| **Max Throughput (64-byte packets)** | **14.8 Mpps per core** (10GbE line rate) | 4.2 Mpps (Scalar iptables) |
| **WASM Boundary Overhead** | **$< 0.05$ ns per packet** (Vectorized) | 8–15 ns per packet (Scalar invocation) |
| **Fault Recovery Time** | **$< 5$ microseconds** (Catch trap & bypass) | System crash / Kernel panic (Flawed C modules) |
| **Hot Reload Latency** | **Atomic (0 frame drop)** | Connection drop or multi-second reload stalls |

---

## 9. Implementation Roadmap

### Phase 1: Core VPP Graph Framework in `vswitchd`

* Implement the vector graph engine (`VectorBatch`, `GraphNode`, `GraphContext`) in Rust inside `services/vswitchd`.
* Build native baseline graph nodes: `hw_nic_rx`, `ethernet_input`, `ip4_lookup`, `vport_dispatch`.
* Verify 10GbE batch forwarding across shared VMO packet rings to `netstack`.

### Phase 2: In-Process Wasmtime Integration

* Embed `wasmtime` (configured with Cranelift AOT compilation and epoch interruption) into `vswitchd`.
* Implement the vectorized ABI bridge passing `VectorBatch` slices into WebAssembly linear memory.
* Build a sample stateful firewall extension in Rust (`examples/firewall.rs`) compiling to `wasm32-wasip1`.

### Phase 3: Dynamic Delivery & Integration with `pkgd`

* Define the `bexos.net.filter.v1` artifact specification in Bazel.
* Integrate `SwitchController` FIDL with `pkgd` to support verified on-demand plugin pulling.
* Benchmark line-rate 100GbE vector processing with concurrent WASM rule swaps in QEMU/KVM and bare metal.
