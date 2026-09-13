# RFC-0054: Unified Media Subsystem (Image, Audio, Video), Dynamic `pkgd` Codec Fetching, and Resource Accounting

* **Author:** BexOS Media & Platform Security Working Group
* **Created:** 2026-09-04T08:47:27-05:00
* **Updated:** 2026-09-13T09:53:12-05:00
* **Status:** In Review / Implementation
* **Target Subsystems:** `mediad`, `pkgd`, `scened`, `devhost`, `lib/oxideav`, `libs/ui`
* **Applicability:** Native Apps, Dioxus Host UI Runners, Servo Web Engine, Camera/Audio Services

---

## 1. Summary

This RFC specifies the unified media decoding/encoding architecture for BexOS. It establishes:

1. **Unified Multi-Modal Pipeline:** A single abstraction layer covering **image rasterization**, **audio transformation**, and **video streaming**.
2. **Broker Matchmaking (`mediad`):** Dynamic selection between hardware ASIC/VPU blocks and isolated software codec workers.
3. **Dynamic On-Demand Fetching via `pkgd`:** Resolution and ingestion of signed, sandboxed OCI codec packages without system restarts or ambient network capabilities in media processes.
4. **Zero-Copy Memory Distribution:** Frame/packet transport executed entirely through read-only and cyclically managed Virtual Memory Objects (`zx.Handle:VMO`).
5. **Caller Resource Attribution:** Execution of software codec workers inside the client's delegated child job hierarchy to enforce CPU, memory, and power accounting.

---

## 2. Scope & Supported Media Modalities

The media pipeline handles three discrete media domains under the standardized `bexos.media` protocol suite:

| Modality | Target Formats & Profiles | Acceleration Engine | Buffer Format |
| --- | --- | --- | --- |
| **Video** | AV1, HEVC/H.265, AVC/H.264, VP9, VVC | SoC VPU (Vulkan Video / MPP) or `codec_worker` | Multi-plane NV12 / P010 / I420 VMO rings directly scannable by `scened` |
| **Audio** | Opus, AAC, FLAC, MP3, Vorbis, PCM, AC-4 | DSP firmware offload or `codec_worker` | Interleaved/Planar f32 or s16 PCM VMO ring buffers directed to `audiod` |
| **Still Image** | JPEG-XL (JXL), AVIF, WebP, PNG, SVG, RAW | GPU Compute / NPU or `codec_worker` | Linear RGBA8888 / RGBA16F / BGRA8888 single-frame VMO buffers |

Parsing container structures (MP4, MKV, WebM, Ogg, RIFF) occurs within the untrusted client application address space. Only raw elementary bitstreams or single-image frames cross the IPC boundary to `mediad`.

---

## 3. System Topology & Architecture

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ APPLICATION RUNTIME (Dioxus Native / Browser / Camera / Image Viewer)       │
│ • Demuxes container format locally                                          │
│ • Holds client-end of `CodecSession` and shared VMO rings                   │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ FIDL: `mediad.CreateCodecSession(...)`
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ `mediad` (Central D1 Media Codec Broker)                                    │
│ • Validates formats (FourCC, profile, sample rate, pixel format)            │
│ • Coordinates Hardware vs. Software fallback                                │
│ • Discovers missing codecs via `pkgd`                                       │
└──────────┬───────────────────────────┬───────────────────────────┬──────────┘
           │ Hardware Path             │ Fetch Plugin              │ Software Spawn
           ▼                           ▼                           ▼
┌───────────────────────┐   ┌───────────────────────┐   ┌─────────────────────┐
│ `vpu_driver` (D1)     │   │ `pkgd` (OCI/TUF)      │   │ `codec_worker` (D2) │
│ • Talks to SoC VPU    │   │ • Dynamic fetch of    │   │ • Spawned into      │
│ • Vulkan Video direct │   │   unsigned/third-party│   │   client's Job      │
│ • DMA to display plane│   │   `.bex` codec plugins│   │ • Pure Rust OxideAV │
└───────────────────────┘   └───────────────────────┘   └─────────────────────┘

```

---

## 4. Dynamic Codec Discovery & Fetching via `pkgd`

To avoid packaging every legacy or proprietary format into base system images, `mediad` integrates with `pkgd` using `libpkg_client`. Neither `mediad` nor `codec_worker` is granted direct network socket access.

```
App                     `mediad`                 `pkgd`               OCI Registry
 │                          │                       │                      │
 │ 1. RequestCodec("jxl")   │                       │                      │
 ├─────────────────────────►│                       │                      │
 │                          │ 2. Check Local Cache  │                      │
 │                          │    (Plugin Missing)   │                      │
 │                          │                       │                      │
 │                          │ 3. ResolveArtifact()  │                      │
 │                          ├──────────────────────►│                      │
 │                          │                       │ 4. Fetch Layer Blob  │
 │                          │                       ├─────────────────────►│
 │                          │                       │◄─────────────────────┤
 │                          │                       │                      │
 │                          │                       │ 5. Validate TUF Sig  │
 │                          │                       │    & RPMB Rollback   │
 │                          │ 6. Return R/O VMO     │                      │
 │                          │◄──────────────────────┤                      │
 │                          │                       │                      │
 │                          │ 7. Spawn `codec_worker`                      │
 │                          │    using mapped plugin VMO                   │
 │ 8. Session Ready         │                                              │
 │◄─────────────────────────┤                                              │

```

### 4.1 Resolution Flow

1. **Cache Evaluation:** Upon receiving a request for an unbundled codec (e.g., `image/jxl` or `video/vvc`), `mediad` scans its local plugin table.
2. **Registry Lookup:** On a miss, `mediad` dispatches an `ArtifactQuery` to `pkgd` specifying `ArtifactKind::DRIVER` or `ArtifactKind::MEDIA_CODEC` for the target FourCC/MIME identifier.
3. **Integrity & Verification:** `pkgd` downloads the signed OCI layer blob, verifies its integrity against the TUF `targets.json` manifest, and ensures version monotonicity against `trusty` RPMB counters.
4. **Read-Only Memory Handoff:** `pkgd` strips write permissions and returns a `zx.Handle:VMO` containing the compiled plugin binary to `mediad`.
5. **Worker Dynamic Loading:** `mediad` injects the plugin VMO into the ephemeral `codec_worker` address space via dynamic linker primitives.

---

## 5. Sandboxing, Isolation, and Memory Safety

Processing untrusted image, audio, and video streams represents a critical vulnerability surface. BexOS applies strict defense-in-depth:

### 5.1 Memory-Safe Pure-Rust Baseline (OxideAV)

Software codecs are built on the **OxideAV** framework (`oxideav-core`, `oxideav-codec`, `oxideav-image`):

* **No C/FFI Surface:** Eliminates traditional memory safety vulnerabilities (buffer overruns, double-frees, integer overflow truncations) pervasive in legacy C codebases.
* **Unified Interface:** Every codec implementation conforms to the standard `BexCodecPlugin` trait:
```rust
pub trait BexCodecPlugin {
    fn init(params: &CodecParameters) -> Result<Box<dyn CodecInstance>, CodecError>;
    fn caps() -> CodecCapabilities;
}

```



### 5.2 Ephemeral Sandboxed Worker (`codec_worker`)

When software decoding executes:

* **No Network Capabilities:** Sockets and network stack access handles are absent.
* **No Filesystem Access:** Direct filesystem operations are blocked; all assets (shared libraries, parameters, buffers) are provided via read-only VMOs.
* **No Hardware MMIO:** Direct access to physical memory or peripheral bus registers is blocked.
* **Attenuated Boundary:** The worker possesses only its input packet VMO, output frame pool VMO, and the control channel.

---

## 6. Zero-Copy Shared Memory Buffer Architecture

Moving raw uncompressed 4K video (approx. 30 MB per NV12 frame), high-resolution camera images (50–100 MB per RAW/RGBA frame), or high-frequency audio buffers via copy operations degrades performance. BexOS utilizes pinned, zero-copy VMO buffer pools.

```
┌────────────────────┐                                     ┌─────────────────┐
│ Client Application │                                     │ `codec_worker`  │
│  or `scened`       │                                     │  (or HW VPU)    │
└─────────┬──────────┘                                     └────────┬────────┘
          │                                                         │
          │ 1. Allocates Shared VMO Frame Pool                      │
          ├────────────────────────────────────────────────────────►│
          │                                                         │
          │ 2. Submits Compressed Bitstream / Image Packet (VMO)    │
          ├────────────────────────────────────────────────────────►│
          │                                                         │
          │ 3. In-Place Decoding: Writes directly into target plane │
          │    (Zero CPU memcpy; direct hardware DMA if ASIC)       │
          │                                                         │
          │ 4. Frame Available Notification (Index #, PTS, Fence)   │
          │◄────────────────────────────────────────────────────────┤
          │                                                         │
          │ 5. Scanout Buffer handed to `scened` without copy       │
          ▼                                                         ▼

```

* **Video:** Cyclical ring buffers (4–8 frames) allocated as shared VMOs. Output buffers map directly into `scened` display pipeline view tokens.
* **Audio:** Ring buffer topologies managed via shared memory channels between `codec_worker`, the application, and `audiod`.
* **Images:** Single discrete VMO allocations sized precisely to image dimensions, returned to the UI layout engine as directly bindable GPU texture surfaces.

---

## 7. Resource Accounting & Denial-of-Service Prevention

To prevent background or malicious applications from exhausting system-wide compute resources by scheduling heavy decoding tasks in a shared daemon, `codec_worker` processes execute inside the **caller's resource hierarchy**.

### 7.1 Delegated Resource Capability

The client application creates a restricted child job token inheriting its scheduling budget and memory quota:

```rust
// Client allocates an attenuated sub-job token
let (child_job, client_token) = current_job.create_child_job(
    JobPolicy::InheritLimits,
    Rights::MANAGE_PROCESS | Rights::TRANSFER,
)?;

```

### 7.2 Enforcement Guarantees

* **Fair-Share CPU Scheduling:** CPU time consumed by software decoding (e.g., software AV1 or JPEG-XL decoding) is billed directly to the requesting application's CPU share.
* **Accurate Battery & Power Attribution:** Energy monitoring diagnostics attribute power consumption to the client application rather than `mediad`.
* **Memory Quotas & Fate-Sharing:** Worker allocations count toward the client application's memory limit. If the worker encounters an out-of-memory condition or leaks buffers, the client's job tree is terminated by the microkernel OOM manager without destabilizing `mediad` or adjacent services.
* **Immediate Cleanup:** If the client crashes, terminates, or is closed, the kernel automatically tears down the associated child job hierarchy, terminating the `codec_worker` instantly.

---

## 8. Interface Definition Language (FIDL)

The core contract resides in `idl/bexos/media/codec.fidl`:

```fidl
library bexos.media;

using bexos.kernel;

type MediaType : uint8 {
    VIDEO = 1;
    AUDIO = 2;
    IMAGE = 3;
};

type CodecDirection : uint8 {
    DECODE = 1;
    ENCODE = 2;
};

struct CodecFormat {
    media_type MediaType;
    fourcc string:8;            // e.g. "AV01", "HEVC", "OPUS", "JXL "
    profile_level uint32;
    width uint32;               // Video / Image
    height uint32;              // Video / Image
    pixel_format uint32;        // NV12, RGBA, etc.
    sample_rate uint32;         // Audio
    channels uint16;            // Audio
};

@discoverable
protocol CodecFactory {
    /// Requests a new isolated codec session
    CreateSession(resource struct {
        format CodecFormat;
        direction CodecDirection;
        resource_token zx.Handle:JOB; // Delegated child job
        input_vmo zx.Handle:VMO;
        output_pool_vmo zx.Handle:VMO;
    }) -> (resource struct {
        session_channel zx.Handle:CHANNEL;
    }) error bexos.kernel.Status;

    /// Queries platform capabilities across hardware and available software plugins
    QueryCapabilities(struct {
        media_type MediaType;
    }) -> (struct {
        supported_formats vector<CodecFormat>:64;
    });
};

protocol CodecSession {
    /// Notify worker of new input buffer slice
    QueueInput(struct {
        offset uint64;
        length uint64;
        pts uint64;
        flags uint32;
    }) -> () error bexos.kernel.Status;

    /// Pushed by worker when an output frame/buffer is fully rendered
    -> OnFrameReady(struct {
        buffer_index uint32;
        pts uint64;
        duration uint64;
    });

    /// Flush buffers during seek operations
    Flush() -> () error bexos.kernel.Status;
};

```

---

## 9. Implementation Roadmap

### Phase 1: Core Protocols & `mediad` Scaffolding

* Finalize `bexos.media` FIDL definitions for Image, Audio, and Video endpoints.
* Stand up `mediad` broker managing hardware VPU dispatch and ephemeral worker instantiation.
* Implement job delegation handshakes enforcing child job sandboxing.

### Phase 2: OxideAV Software Decoders & VMO Rings

* Integrate pure-Rust `oxideav-codec` (video), `oxideav-image` (JXL/AVIF), and `oxideav-audio` (Opus/AAC) into standard dynamic plugin shared objects.
* Implement zero-copy buffer pool negotiation between `mediad`, `codec_worker`, and client apps.
* Wire decoded image outputs directly into Dioxus host rendering contexts.

### Phase 3: Dynamic Packaging via `pkgd`

* Define the `bexos.media.codec` OCI packaging format.
* Connect `mediad` to `pkgd` via `libpkg_client` to resolve missing codecs on demand.
* Implement verification policies for downloaded software codec packages before mounting into `codec_worker`.