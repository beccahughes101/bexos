# RFC 0056: BexOS Workplace remote sessions

- Created: 2026-09-04T17:15:21-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

BexOS Workplace combines headless composition, media streaming, peripheral forwarding, and enterprise identity for cloud-independent remote workstations. The design also describes its deployment model and intended economic benefits.

## Design overview

**BexOS Workplace** is a cloud-independent remote workstation design comparable to Citrix-style virtual desktops. It uses BexOS’s existing architectural boundaries.

Traditional Virtual Desktop Infrastructure (VDI), such as Citrix HDX, VMware Horizon, and AWS WorkSpaces, runs full desktop operating systems (Windows Server or monolithic Linux) in Type-1/Type-2 VMs. The design addresses the resulting RAM requirements, limits on host density, GPU virtualization licensing, and attack surface.

Because BexOS is a capability-secure, resource-isolated microkernel with native support for headless operation, hardware-accelerated rendering (`scened`), zero-copy media streaming, and sandboxed execution, it can deliver high-performance remote sessions with lower overhead.

## High-Level Architecture: BexOS Workplace

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ CLIENT DEVICES (Mac, Windows, iPad, Android, Thin Client, BexOS Hardware)   │
│ Client: Native WebRTC / WASM Browser Client OR Thin BexOS Client App        │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ Secure Encrypted Channel (QUIC / WebRTC)
                                       │ • AV1 / HEVC video stream
                                       │ • Low-latency input events (Touch, HID)
                                       │ • Bi-directional audio & USB redirection
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ ANY CLOUD PROVIDER (Bare Metal, AWS EC2, GCP, Azure, Hetzner, Equinix)      │
│ Orchestrator: Lightweight Control Plane (K8s / Nomad + OCI Distribution)    │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ Launches isolated user instances
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ BEXOS WORKPLACE INSTANCE (MicroVM / Enclave / Container-Level Isolation)    │
│                                                                             │
│  [ scened-headless (Virtual Compositor) ] ──► [ Direct GPU / SwiftShader ]  │
│         │                                                                   │
│         ▼ Raw Frame VMOs                                                    │
│  [ mediad + OxideAV / Vulkan Video Encoder ] ──► AV1/HEVC Hardware Stream   │
│         │                                                                   │
│         ▼ Encoded Packets                                                   │
│  [ remoted (Workplace Transport Gateway) ] ──► Multiplexed QUIC/WebRTC      │
│         ▲                                                                   │
│         │ Input Events / USB Forwarding                                     │
│  [ usbd (Virtual USB Host) / inputd ]                                       │
│                                                                             │
│  ═════════════════════════════════════════════════════════════════════════  │
│  USER SCOPE & APPLICATION RUNTIME:                                          │
│  • BexOS Native & WASM App Sandboxes (`user_created:<uid>:`)                │
│  • Isolated Android MicroVM (pKVM) for mobile app parity                    │
│  • Linux/Proton MicroVM for legacy desktop software                         │
│  • Hardware-isolated Keystore via Trusty (VTL1 / Hypervisor Enclave)        │
└─────────────────────────────────────────────────────────────────────────────┘

```

## Cloud deployment economics

### Instant Density & Low Idle RAM Footprint

A Windows 11 VDI VM demands 4–8 GB of base RAM just to stay idle. A minimal BexOS microkernel instance with essential D1 services (`appd`, `scened-headless`, `mediad`, `fshost`, `teed`) can boot in under **100 milliseconds** and idle at **under 150 MB of RAM**. A single bare-metal cloud server (e.g., 64 cores, 256 GB RAM) can host 50–100 active BexOS Workplace sessions where traditional VDI hosts only 15–25.

### Cloud-Agnostic Bare-Metal or VM Deployment

Most modern cloud VDI products lock customers in:

* AWS WorkSpaces requires AWS infrastructure and Active Directory.
* Azure Virtual Desktop (AVD) requires Azure subscriptions and Entra ID.
* Citrix requires deep Windows licensing and proprietary management servers.

BexOS Workplace images package directly as **OCI Artifacts** through the existing distribution pipeline. Enterprises can deploy Workplace instances on:

* **Public Hyperscalers:** AWS, Azure, Google Cloud (via standard KVM/hypervisor instances).
* **Commodity / Sovereign Clouds:** Hetzner, OVH, Equinix Metal, or private on-prem OpenStack/Proxmox clusters.
* **Edge Nodes:** Direct office servers for ultra-low-latency on-prem CAD/dev workstations.

## Key Subsystems for BexOS Workplace

### A. Virtual Scanout & The Zero-Copy Encoding Pipeline

Citrix uses proprietary display drivers (Citrix Indirect Display / Thinwire). BexOS implements display streaming in userspace:

1. **`scened-headless`:** A headless variant of the Flatland display compositor. Instead of flipping frames to an HDMI/eDP display plane, it composites directly into a **ping-pong buffer pool of uncompressed VMOs**.

#### Hardware/Software Streaming Bridge

* The compositor hands the filled frame VMO to `mediad`.
* If a cloud GPU is attached (NVIDIA vGPU, AMD, Intel Flex), `mediad` invokes Vulkan Video / hardware encoder pipelines.
* If on a CPU-only cloud instance, `mediad` delegates encoding to an **OxideAV-based AV1 or H.264 real-time software encoder** running inside a dedicated worker.

3. **Transport via WebRTC / QUIC:** A D1 gateway daemon (`remoted`) streams the elementary video stream over a datagram-based QUIC connection with congestion control tuned for interactive framerates (60–120 FPS).

### B. Input & Virtual Peripheral Redirection (`usbd` Extension)

Client peripheral forwarding uses the `usbd` client-driver capability model:

* **Tablet Mode (Touch & Stylus):** The remote client (e.g., an iPad running the Workplace web client) captures native touch coordinates, pressure levels, and tilt, sending them over the QUIC channel. `remoted` pushes them directly into BexOS’s `touch_driver` or `inputd`.
* **USB-over-IP Redirection:** If a user plugs a YubiKey, flash drive, or smartcard reader into their physical laptop, the local client tunnels the USB descriptors over the network. On the cloud instance, `usbd` binds to the virtual stream, allowing existing D1 USB class drivers to operate without knowing the hardware is remote.

### C. Enterprise Security & Dynamic Device Identity

Enterprises care about **data exfiltration prevention**:

* **Enclave-Sealed Sessions:** Every Workplace instance boots with an ephemeral **Trusty TEE (VTL1)** enclave. Credentials, corporate certificates, and user-generated signing tokens never touch the host cloud provider's unencrypted memory space.
* **Watermarking & Clipboard Policy:** Because `scened` and `compositor` own the presentation pipeline, `scened` can inject hardware-unremovable, subtle user-session watermarks directly into the video stream to prevent screen photography leaks.
* **No Local Disk Persistence:** Enterprise administrators can configure the root filesystem as a read-only verified block device (authenticated via TUF), storing ephemeral user changes in an encrypted memory overlay that zero-wipes on session termination.

## Client Access: Web-First + Native Thin Client

To make adoption frictionless, avoid requiring users to install complex proprietary client software on day one:

### Web Browser (Zero-Install Access)

* Provide a client written in **Rust + WASM** using standard browser APIs:
* **WebRTC / WebTransport** for low-latency audio/video streaming.
* **WebCodecs API** for hardware-accelerated video decoding in Chrome/Safari/Edge.
* **Pointer Lock & Touch Events** for native mouse and tablet stylus tracking.

* A user on an iPad, Chromebook, or locked-down corporate Windows laptop navigates to [https://workplace.company.com](https://workplace.company.com), logs in via SSO, and immediately gets an interactive BexOS workstation in a browser tab.

### Native BexOS Thin Client (For Dedicated Terminals)

* A stripped-down BexOS hardware image running on cheap ARM boards (e.g., RK3588, Raspberry Pi, or repurposed old PCs) acting as an enterprise thin client, booting straight into the remote Workplace session.

The design addresses the economic and security tradeoffs of two common enterprise VDI architectures:

| Platform | The Core Pain Points | The Economic & Security Reality |
| --- | --- | --- |
| **Windows VDI** (AVD, Citrix, WorkSpaces) | Licensing costs (VDA licenses, RDS CALs, Windows Server / Enterprise markup); 4–8 GB RAM idle per user; limited density per hypervisor node. | Enterprises pay for the OS baseline before running business software. |
| **Linux Desktops** (VNC, RDP, Container-VDI) | Ambient authority (`root` versus user); additional security layers (SELinux, AppArmor, seccomp) that can break applications; X11/Wayland authorization leaks and complex IPC permissions. | Lower hosting costs, with CISO and SecOps concerns about privilege escalation in mixed enterprise workloads. |

BexOS Workplace aims to combine **lower cloud costs than Linux** with a **stronger security posture than Windows**.

## Density and licensing

Without Microsoft licensing fees or Windows kernel bloat, cloud hosting margins shift dramatically:

* **Zero OS Licensing Tax:** BexOS runs on open, proprietary-free foundational plumbing. No CALs, no per-core OS licensing surcharges, and no cloud-vendor lock-in.

### 10x Instance Density

* A standard 64-core, 256 GB RAM cloud bare-metal node (e.g., on AWS, Hetzner, or Equinix) maxes out around **20–30 Windows VMs** before hitting memory exhaustion.
* Because a headless BexOS session idles at **~100–150 MB RAM** with near-instant sub-second boot times, that identical server can pack **100–150 active BexOS Workplace instances**.

* **Dynamic Ephemeral Spawning (Just-In-Time VDI):** Instead of paying for persistent instances idling 24/7, instances spawn on-demand in milliseconds when a user logs in via the browser and terminate on logout.

## Security architecture

Linux's security problem is structural: it was designed in the 1970s/1990s around POSIX ambient authority. If a process gains access to a UID, it inherits wide read access across `/proc`, `/sys`, IPC sockets, and unconfined filesystem trees. Hardening Linux requires stacking brittle layers (cgroups, namespaces, seccomp filters, SELinux policies) that administrators frequently disable because they break workflows.

BexOS solves this at the microkernel and capability layer:

### No Ambient Authority

A BexOS process cannot open a file, talk to another service, or touch hardware because it runs as a specific user. It can only talk to channels explicitly handed to it at spawn. If a web browser or rendering worker inside BexOS Workplace is exploited, the attacker is confined to an empty handle table—they cannot probe the network, inspect other processes, or escalate to a universal "root."

### Stage-2 Isolation & Enclave-Sealed Identity

* Each Workplace instance runs with its own isolated **Trusty TEE (VTL1)** enclave.
* Enterprise secrets (corporate SSO tokens, TLS private keys, decrypted session credentials) are held in memory that is unmapped and cryptographically protected from both other tenants and the host cloud infrastructure.

### Read-Only Base with Ephemeral Overlays

* The operating system root is a cryptographic, read-only block tree verified via TUF.
* Malware or persistent threats cannot modify the base OS. Any temporary files or local cache exist in an encrypted, ephemeral RAM/VMO overlay that is securely wiped the second the user disconnects. Every session starts from an immutable, bit-for-bit verified state.

## Intended enterprise benefits

The intended benefits for enterprise stakeholders are:

1. **Finance:** Reduce cloud VDI spending by 60–75% through removal of Microsoft licensing overhead and 4x more users per server.
2. **Security:** Eliminate the Linux attack surface, including `sudo`, shared root filesystems, and ambient kernel privileges; enforce memory isolation for every tenant in hardware.
3. **End users:** Open a secure workspace from an iPad, Chromebook, or personal laptop in under 2 seconds through a browser, without proprietary VPN clients or heavy agents.
