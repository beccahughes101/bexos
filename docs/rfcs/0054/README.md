# RFC 0054: Media codecs and resource accounting

- Created: 2026-09-04T08:47:27-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

Mediad selects isolated hardware or software codec workers and shared buffer pools. Delegated resource capabilities charge decoding work to the requesting application.

## Design overview

Media processing uses a **central mediation service** with **dynamically loaded driver and codec packages**. Media decoders are historically among the largest attack vectors on mobile and desktop OSs; processing untrusted, malformed streams inside isolated sandbox domains prevents kernel compromise or app privilege escalation.

A pure-Rust framework such as **OxideAV** (`oxideav-core`, `oxideav-codec`) provides a memory-safe, C-free baseline that eliminates buffer overflows and use-after-free vulnerabilities common in legacy C codebases like FFmpeg.

## System Architecture: The Media Pipeline

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ BEXOS APPLICATION CLIENT (e.g., Browser, Camera, Video Player)              │
│ Demuxes container, acquires encrypted or raw elementary stream              │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ FIDL: `bexos.media.CodecSession`
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ `mediad` (Central D1 Media Codec Broker)                                    │
│ • Negotiates formats (FourCC, profile/level, colorspace, sample rate)       │
│ • Matchmaking: Hardware ASIC vs. Sandboxed Software fallback                │
│ • Manages zero-copy shared memory buffer pools (`VMO` rings)                │
└───────────────────┬─────────────────────────────────────┬───────────────────┘
                    │ Direct Driver Handle                │ Spawns Worker
                    ▼                                     ▼
┌──────────────────────────────────────┐ ┌────────────────────────────────────┐
│ HARDWARE ACCELERATION DRIVER         │ │ `codec_worker` (Ephemeral Sandbox) │
│ (`vpu_driver` in D1 / Vulkan Video)  │ │ • Isolated, restricted D2 Sandbox  │
│ • Talks to SoC VPU / NVDLA blocks    │ │ • Restricted caps: NO net, NO fs   │
│ • Zero-copy DMA directly to display  │ │ • Dynamically loads codec plugin   │
│   plane or camera ISP                │ │   (e.g., `liboxide_h265.so`)       │
└──────────────────────────────────────┘ └────────────────────────────────────┘

```

## Hardware vs. Software Dispatch

The central broker (`mediad`) implements fallback logic based on device capabilities and power profiles:

```
[ App requests Decoder (e.g., HEVC / AV1 / H.264) ]
                        │
                        ▼
          Does SoC VPU support profile?
                     /     \
             YES    /       \   NO
                   ▼         ▼
       [ Route to `vpu_driver` ]   [ Spawn `codec_worker` ]
       • Direct MMIO to ASIC /     • Sandboxed process
         Vulkan Video VideoSession • Load OxideAV software decoder
       • Lowest latency & power    • Memory-safe decoding in Rust

```

* **Hardware Codecs (VPU / GPU):** Handled via BexOS's hardware driver interface (using Vulkan Video extensions or SoC-specific VPU drivers like Rockchip MPP or Google Tensor VPU). `mediad` routes decoded frames straight to display planes via scanout VMOs.
* **Software Codecs (OxideAV):** Used when the hardware lacks support for specific profiles (e.g., AV1 10-bit on older chips, niche audio codecs like FLAC/Opus/AC-4, or image formats like WebP/JPEG-XL).

## Sandboxing & Dynamic Plugin Packages

Never load third-party or untrusted software codecs directly into the `mediad` broker process address space. A malformed container or exploit could bring down the system-wide media service.

* **Ephemeral `codec_worker` Instances:** When software decoding is required, `mediad` launches an isolated `codec_worker` process with an attenuated capability token:
* **No Filesystem Access:** Cannot read or write arbitrary files; packages are provided as read-only, mapped VMOs.
* **No Network Capabilities:** Cannot communicate outside its assigned IPC channel.
* **No Device MMIO:** Memory access is strictly limited to input/output packet buffers.

* **Packaging as OCI Artifacts / `.bex`:** Codecs (e.g., `pkg:/codecs/oxide-av1.bex`) are distributed as standard BexOS packages. `mediad` loads the plugin `.so` via dynamic linker primitives into the restricted `codec_worker`.

## Pure-Rust Codec Engine: OxideAV Integration

OxideAV's modular, zero-C structure maps directly onto BexOS:

* **No C ABI / FFI Hazards:** Upstream crates (`oxideav-core`, `oxideav-codec`, `oxideav-aac`, `oxideav-vp9`) implement decoding logic in safe Rust without `*-sys` wrappers, eliminating entire classes of memory safety bugs.

### Modular Trait Implementations

Each codec plugin implements the standardized `oxideav_codec::Decoder` / `Encoder` trait interface:
```rust
pub trait BexCodecPlugin {
    fn init(params: &CodecParameters) -> Result<Box<dyn Decoder>, CodecError>;
    fn caps() -> CodecCapabilities;
}

```


* **Separation of Parsing & Execution:** OxideAV separates container demuxing (`oxideav-container`) from frame decompression (`oxideav-codec`). Demuxing can execute in userspace directly inside the client application, while raw elementary bitstream packets are fed across the VMO boundary to the decoder.

## Zero-Copy Media Buffer Architecture (`VMO` Pool)

Moving uncompressed 4K video frames (approx. 25–35 MB per raw NV12/RGBA frame) over IPC creates high CPU and cache overhead. Decoding must use **zero-copy shared memory rings**:

```
 ┌──────────────┐                                       ┌────────────────┐
 │ Client / UI  │                                       │ `codec_worker` │
 │  Compositor  │                                       │  (or HW VPU)   │
 └──────┬───────┘                                       └────────┬───────┘
        │                                                        │
        │ 1. Allocates Shared VMO Frame Pool (Cyclic Buffer)     │
        ├───────────────────────────────────────────────────────►│
        │                                                        │
        │ 2. Sends Compressed Bitstream Packet (VMO offset)      │
        ├───────────────────────────────────────────────────────►│
        │                                                        │
        │ 3. Decodes directly into target uncompressed VMO frame │
        │    (Zero CPU memcpy, DMA-direct if HW)                 │
        │                                                        │
        │ 4. Completion Signal (Frame Ready: index #2, PTS)      │
        │◄───────────────────────────────────────────────────────┤
        │                                                        │
        │ 5. Hands VMO handle straight to `scened` for Scanout   │
        ▼                                                        ▼

```

1. **Buffer Allocation:** The client (or `scened`) creates a contiguous or pinned `VMO` containing a buffer ring (e.g., 4–8 frames for video playback smoothing).
2. **Buffer Registration:** The client passes read/write capabilities for the frame pool to `mediad` (which delegates it to the decoder).
3. **In-Place Decoding:** The decoder—whether software via OxideAV or hardware via VPU DMA—writes decoded pixel planes directly into the assigned frame slot.
4. **Direct Presentation:** The client hands the frame identifier directly to `scened` (Flatland compositor), displaying the frame with zero intermediate memory copies.

## Recommended Implementation Roadmap

1. **Define `bexos.media` FIDL Interfaces:** Specify protocols for `CodecFactory`, `CodecSession`, `InputPacketStream`, and `FrameBufferPool`.
2. **Implement `mediad` Daemon:** Build the core registry service handling format matching, buffer lifecycle, and worker spawning.
3. **Build the OxideAV Software Bridge:** Wrap core OxideAV crates (`oxideav-aac`, `oxideav-h264`, `oxideav-opus`) into a shared Rust dynamic plugin format targeting the `BexCodecPlugin` ABI.
4. **Connect Hardware VPU Drivers:** Expose SoC hardware video decoding through Vulkan Video or direct VPU driver bridges, ensuring identical VMO output formats between software and hardware engines.

## Resource attribution and denial-of-service prevention

Codec workers must account for resource use on behalf of their callers so that applications cannot exhaust a shared service’s budget.

In many microkernel and monolithic designs, centralized servers (like an audio or media daemon) perform CPU- and memory-heavy decoding inside their own scheduling domain or process pool. This allows a malicious or runaway background app to exhaust the media server's budget, starving foreground apps and breaking scheduling fairness.

Assigning the worker to the caller's resource context directly mirrors patterns proven in modern capability systems—such as **Fuchsia's Job/Resource hierarchy** and **seL4's Thread Control Block (TCB) / CNode delegation**.

## The Architecture: Delegated Resource Capability

Instead of giving `mediad` a blank check to bill arbitrary resources, the client issues a **scoped, attenuated capability token** representing its resource group (e.g., a restricted `zx.Handle:JOB` or `zx.Handle:RESOURCE_GROUP`).

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ CALLING APP (e.g., Video Player / Browser)                                  │
│ Holds:                                                                      │
│   • Child Job Creation Token (`RESOURCE_GROUP_DELEGATE`)                    │
│   • Input bitstream VMO & Output frame pool VMO                             │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ FIDL: `mediad.CreateCodecSession(job_cap, ...)`
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ `mediad` (Central Broker — Runs in System Group)                            │
│                                                                             │
│ 1. Receives client's delegated `job_cap`                                    │
│ 2. Spawns `codec_worker` directly inside the caller's Job / Cgroup           │
│ 3. Attenuates worker permissions:                                           │
│    - Strips network, filesystem, and driver MMIO access                     │
│    - Attaches strictly input/output VMOs and TIPC session port              │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ Spawns into Caller's Hierarchy
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ CALLER'S RESOURCE GROUP / JOB HIERARCHY                                     │
│                                                                             │
│  [ Parent App Process ]             [ Sandboxed `codec_worker` ]            │
│  • UI Thread, Demuxer, App Logic    • OxideAV software decoder plugin       │
│                                                                             │
│  ═════════════════════════════════════════════════════════════════════════  │
│  COMMON KERNEL ENFORCEMENT BOUNDARY:                                        │
│  • Shared CPU Time Slice & Thread Priority (Fair-share / Deadline)          │
│  • Shared Memory Quota (Killed by OOM together if memory leaks)             │
│  • Shared Power & Battery Accounting                                        │
└─────────────────────────────────────────────────────────────────────────────┘

```

## Key Technical Benefits

* **Zero-Budget System Daemon:** `mediad` itself consumes virtually no memory or CPU beyond message routing. It is merely a factory/broker, making it immune to compute exhaustion.

### Accurate Resource & Battery Accounting

* If a background app attempts 4K AV1 software decoding, the CPU time is billed directly to that app’s scheduling group.
* The OS battery and CPU meters accurately attribute the power consumption to the player or browser tab, rather than a misleading "System Media Server" entry.

### Fate-Sharing and Automatic Cleanup

* If the caller crashes, hangs, or is terminated by the user, the kernel tears down the entire Job/Group tree. The `codec_worker` dies instantly with the parent app, leaving no orphaned decoders running in the background.
* If the decoder leaks memory, the memory limit of the *app* is hit, triggering the OOM killer on the rogue workload without impacting the rest of the OS.

### Sandboxed Principle of Least Privilege

* Even though the worker lives within the app's resource group for accounting, `mediad` strips all functional capabilities before launching it: no file access, no socket access, no direct hardware DMA access. The worker is mathematically limited to decompressing bytes between two memory handles.

## Designing the Capability Protocol

To prevent privilege escalation, the delegated capability must be strictly one-way and unforgeable.

### The Kernel Primitive: Attenuated Sub-Job Token

The calling app does not hand over its *own* root handle. It asks the microkernel to create an attenuated child job handle with the restricted right: `ZX_RIGHT_MANAGE_PROCESS | ZX_RIGHT_APPLY_POLICY`.

```rust
// Calling app creates a restricted delegation token
let (child_job, client_token) = current_job.create_child_job(
    JobPolicy::InheritLimits, // Memory/CPU caps tied to parent
    Rights::MANAGE_PROCESS | Rights::TRANSFER,
)?;

```

### The FIDL Protocol Handshake

```fidl
library bexos.media;

protocol CodecFactory {
    /// App requests a decoder session, providing its execution group capability
    CreateDecoder(resource struct {
        codec_id: string:64;           // e.g. "oxideav.h265"
        resource_token: zx.Handle:JOB; // Delegated child job
        input_vmo: zx.Handle:VMO;      // Bitstream buffer
        output_vmo: zx.Handle:VMO;     // Presentation swapchain
    }) -> (resource struct {
        session_channel: zx.Handle:CHANNEL;
    }) error CodecError;
};

```

### Worker Launch Sequence

1. **Validation:** `mediad` checks that `resource_token` is a valid Job capability and that its memory and priority policies conform to platform limits.
2. **Process Construction:** `mediad` invokes the microkernel's process launch syscall, targeting the provided `resource_token` as the parent job.
3. **Execution & Dropping:** `mediad` transfers the input/output VMOs to the newly minted worker, launches its initial instruction pointer (loading the requested OxideAV plugin), and **closes its own copy of the client's `job_cap`**.
4. Once spawned, `mediad` is completely out of the data path—the client and the worker communicate directly over their dedicated session channel.
