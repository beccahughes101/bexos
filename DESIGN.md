### Goals

- Secure  
  - Everything is signed  
  - Allowlist permission  
- Designed for the modern world  
  - i.e. designed for multi core systems  
- Highly performant and robust  
  - Everything is rust  
  - Everything is components and hot swappable  
- Open source  
- Extendable (e.g. chrome like extensions in any app or the OS itself)

Technology choices:

- Rust  
- Go (where needed, probably server)  
- Connect RPC (client to server using quic)  
- Protobuf (prototxt to compiled buffer)  
- Bazel  
- redb for storage

### Kernel Structure

The kernel will use Rust features to allow the user to pick which features to compile in. It will also have a key value configuration system. It should be setup to be highly pluggable and support multiple architectures.

### Boot (ARM)

ARM systems use TF-A and Trusty. TF-A authenticates the BexOS BL33 verifier,
which validates the AVB metadata and measured boot artifacts before entering the
kernel. Trusty hosts KeyMint, Gatekeeper, secure storage, AVB, AuthMgr FE/BE,
and the “BexOS orchestrator” secure-world application. The orchestrator has the
following responsibilities:

1. Orchestrating heart transplants and multiple kernels

### BexOS Orchestrator

having a list of critical images (e.g. kernel/drivers) and it will keep a list with hash of last known good and then if the system fails a health check it rolls back to last known good this would work for kernel updates and rollback the kernel, but it could also rollback a driver update that crashes the devices

If a new GPU driver update triggers an MMU fault or fails to pass a health check, rolling back the whole OS is unnecessary. The orchestrator swaps out only that specific driver.img, restoring the last-known-good version while leaving the updated microkernel and userspace state intact.  Granular A/B Versioning:  
Instead of monolithic A/B disk partitioning (which duplicates gigabytes of flash), you maintain an append-only content-addressed store of signed blobs (kernel.img, audio.img, gpu.img) alongside a lightweight, authenticated manifest listing active hash references.  Decoupled Failure Domains:  
Driver crashes in D1/D2 user compartments are caught before they contaminate persistent storage, allowing quick component restoration without losing open app data. 

### Heart Transplant

All the components are easily updated, we will support a first class kernel feature called heart transplant for non reboot kernel updates. It will do this:

1. The BexOS orchestrator will load the new kernel in its entirety (no patching), verify the integrity and signature  
2. The old and new kernel will create a communication channel which will sync the state from old to new (stable ABI)  
3. The BexOS orchestrator will switch the old kernel will switch to “SWITCH” mode which stops all program execution, and a lot of unnecessary kernel tasks  
   1. The new kernel will finish syncing and send a completion message to the orchestrator  
   2. The old kernel will stop doing anything and new kernel will take over  
4. Program execution will resume  
5. The old kernel is removed from memory

Decisions

- Both kernels cannot manage physical hardware (MMU registers, GIC interrupt controllers, timers) at the same time without conflicting.  
  - The Fix: The Old Kernel remains the sole hardware owner during the sync phase. The New Kernel runs in a quarantined sandbox (e.g., executing in a pre-allocated physical memory block, communicating strictly via shared-memory rings or IPC, without touching peripheral hardware or rewriting global MMU tables) until the orchestrator officially grants it control.  
- CPU Core Allocation During Sync  
  - The orchestrator runs in Secure EL1 / EL2 (Hypervisor/TEE) and switches execution contexts between old and new kernels until the sync completes.  
- Handling In-Flight Hardware Interrupts & Timers  
  - When entering "SWITCH" mode, the old kernel masks local interrupts, drains all active DMA/IPC queues, and serializes the hardware state (register contexts, pending IRQ affinities) across the sync channel. The new kernel re-programs the interrupt controller base registers to point to its own vector table upon takeover.

two-stage sync:

Stage 1 (Live Bulk Sync): Stream static state (process descriptors, memory regions, file tables) while apps are still running.

Stage 2 (Quiesced Delta Sync in "SWITCH" Mode): Stop scheduling, freeze CPU registers, capture the final capability state, swap control through the Trusty orchestrator, and resume.

**Completely Invisible Update (The Ideal Path)**

* **Behavior:** Do not draw any modal UI or spinner.  
* **User Perception:** Apps experience a tiny \~100ms stutter, after which a subtle desktop toast/notification appears: *"System core updated to v1.2.4 without reboot."*  
* **Advantage:** Reinforces the magic of the OS—it feels as seamless as an invisible garbage-collection pause or frame drop.

Instead of a takeover overlay, the status bar/compositor shows a tiny, non-blocking icon (like a pulsing icon in the corner) during Stage 1 live-sync, which turns into a checkmark once the switch completes.

Panic Trapping: When the non-secure microkernel panics, it triggers a synchronous exception or an explicit SMC to the BexOS orchestrator. If the kernel hard-locks without executing a panic handler, the orchestrator's secure watchdog timer expires and preempts execution. 

Fast In-Place Restart: For isolated software faults (e.g., transient memory exhaustion or unhandled assertions), the orchestrator can re-zero the kernel bss/data segments, reload the identical verified kernel binary, and attempt to restore the checkpointed userspace state.  

Fallback to A/B Slot: If consecutive fast-restarts fail within a set retry threshold or a heart transplant update fails health checks, the orchestrator flags the active slot as unbootable, switches the boot target to the alternate verified A/B slot, and performs a clean fallback boot. 

### Drivers

There will be the following levels of drivers:

1. D0 \- In-kernel rust  
2. D1 \- Out of process mojo rust  
   1. Linux driver compatibility layer  
   2. Networking, etc  
3. D2 \- WASM  
   1. HID

### Services (FIDL fork)

Service Manager is a app-level feature and every process is given a handle to an interface that has the following features:

- Broadcast  
- PublishInterface  
  - With optional KV metadata  
- GetInterfaces  
- GetSingletonInterface  
- GetUserScopedSingleton

### Push Notifications

Encrypted  
Support local using SSID

### App Platform

It will be more like the web:

- Apps hosted by developers on the website  
- App Store is a directory and handles sales  
- All apps must be signed using a standard code signing certificate  
- We will have a global malware list that will do things like safe search lookup and can globally disable malware apps  
- We will have a trust level setting:  
  - High (default)  
    - Must be distributed on domain with [BIMI \+ VMC](https://bimigroup.org/vmc-issuers/) we will show the user an install screen showing this is the “verified publisher”  
      - We will have a central brand list that a company can register by using a VMC signature to create a list of “protected trademarks”, pin domains and app certs  
        - e.g. Monzo adds their logo \+ name to the pinned list  
          - A scammer tries to create a fake monzo app that uses the name/logo, the device on installation will check the app logo and name against the protected trademarks list and only allow if it was created by monzo  
    - Must be signed using a standard code signing certificate  
  - Low  
    - Must be signed using a standard code signing certificate

## Apps

Everything is an app defined by a manifest. If the app is downloaded from the web the package name is prepended with the domain the app was downloaded from e.g. an app from [monzo.com](http://monzo.com) would be \`com.monzo:monzo\`. The manifest is a prototxt (compiled to proto) file and has the following concepts:

- Name  
- Package Name (suffix)  
  - A developer can specify a suffix or full package name e.g. an app downloaded from [dl.google.com](http://dl.google.com) will specify com.google:maps and com.google will be allowed since [google.com](http://google.com) is the parent domain (we will use PSL), in addition a developer can also publish a well known file that includes the following:  
    - Trusted distribution domains (e.g. CDNs or app store domains)  
    - Trusted domains (e.g. [waymo.com](http://waymo.com) will trust [google.com](http://google.com) and vice versa to share data)  
- Processes (executables)  
  - Can have multiple  
    - One default named default  
  - Name  
  - Runner  
    - ELF  
    - WASM  
    - Custom  
  - Permissions  
    - Key value like apple  
    - Required/Desired  
  - Service (i.e. background)  
    - If this should be run as a service when this should be run (e.g. driver phase)  
    - Depend on other services  
  - Link  
    - If this should be shown in the app directory  
    - Icon \+ Name  
- Manifest extensions  
  - String key serialized proto value


well known eg.

\`\`\`  
{  
  "origin": "https://waymo.com",  
  "allowed\_package\_prefixes": \[  
    "com.waymo"  
  \],  
  "trusted\_distribution\_origins": \[  
    "https://cdn.waymo.com",  
    "https://distribution.bexos.org"  
  \],  
  "trusted\_peer\_domains": \[  
    "https://google.com",  
    "https://alphabet.com"  
  \],  
  "signing\_certificate\_fingerprints": \[  
    "SHA256:4a:6f:8b:..."  
  \]  
}

\`\`\`  
The init process is actually the appd. The appd and other core services can be updated through heart transplant to prevent all apps being killed.

- Appds manages all manifests  
  - Device installed  
  - User installed  
- Source of all truth for permission grants  
- Hosts service manager for FIDL  
  - Enforces bind permissions  
  - Broadcast  
    - Permission required w/ channel as value  
  - PublishInterface  
    - With optional KV metadata  
  - GetInterfaces  
    - Filter by KV metadata  
  - GetSingletonInterface  
  - GetUserScopedSingleton  
  - FIDL schema extension for requiring permission  
    - Permissions are stored by appd and not kernel to avoid context switching  
    - FIDL channels between two user space apps never touch the kernel  
    - The appd should only broker/mint a channel endpoint to a service if the requesting app has the required permission grant. Once the channel is connected, the service knows the caller is authorized without verifying per-message permissions on every call.  
    - The developer writes a single protocol and tags methods with annotations like @permission("CAMERA") or @permission("HARDWARE\_CONFIG").  
    - fidlc splits the AST into separate sub-protocols (e.g., Camera\_CameraPermission, Camera\_HardwareConfigPermission, and a default Camera\_Public).  
    - The compiler generates a single Rust server trait for the service implementer containing all methods, automatically routing incoming channel traffic from the respective sub-protocols to that shared implementation.  
    - When an app requests a connection to bexos.hardware.Camera, the appd matches the client app's manifest permissions against the interface manifest.  
    - The appd only creates and passes channel handles for the sub-interfaces the client is cleared to use.   
- Kernel  
  - Exposes FIDL interface to appd, appd proxies to apps and applies permissions

e.g.

\`\`\`  
library bexos.hardware.camera;

protocol CameraController {  
    // Basic method (requires normal capture permission)  
    @permission("android.permission.CAMERA\_RECORD=Camera1")  
    RecordStream(resource struct { client\_sink client\_end:AudioVideoSink });

    // High-privilege method (requires restricted hardware-level permission)  
    @permission("bexos.permission.RAW\_SENSOR\_TUNING")  
    SetSensorRegisters(struct { register\_map vector\<uint8\> });  
};  
\`\`\`

#### Sandboxing

Everything is sandboxed. Permissions (in manifest) define what a process can do. Sandboxes define what a process can access. Resource groups define the pool of resources a process can use (i.e. cgroups).

- Sandboxes (Access Isolation):  
  - By default, every package gets an isolated, encrypted directory root with dedicated capability handles.  
  - When com.google:maps and com.google:photos both request google\_shared\_vault, the appd verifies that both origins share bilateral trust via .well-known/bexos-manifest.json. It then passes the same underlying capability storage handle to both apps.  
- Permissions (Capability Routing):  
  - Permissions gate access to services and FIDL methods rather than static file paths.  
  - Even if an app is part of a shared sandbox, it cannot access device APIs (e.g., GPS, camera) unless explicitly declared in its manifest and granted by the user  
- Resource Groups (Cgroup Allocation):  
  - Resource groups throttle hardware consumption (CPU time slices, memory high/low watermarks, GPU rendering quotas).  
  - Domain-Level Pools: All background tasks belonging to com.google:\* can be assigned to a single domain resource group. If com.google:drive starts a massive sync in the background, it shares and is constrained by the google\_background pool, preventing it from starving the rest of the OS or foreground UI.

e.g.

CEL permissions

- Context Environment: When appd brokers a service bind or checks a capability, it builds an evaluation context:client: Package name, domain, signing cert fingerprint, foreground/background state, trust tier.  request: Requested method ordinal, parameters/metadata, declared intent.device: Battery status, lock state, network connectivity.  
- Static: Channel Bind Time (in appd)  
- Dynamic: Invocation Time (in Server process)

\`\`\`  
library bexos.hardware.camera;

protocol CameraController {  
    // Basic single permission  
    @permission("request.permissions.contains('CAMERA')")  
    CaptureFrame() \-\> (struct { frame vector\<uint8\> });

    // Key-Value Attribute & State checks  
    @permission("request.permissions.contains('CAMERA') && "  
                "(client.is\_foreground || client.domain in \['google.com', 'waymo.com'\]) && "  
                "device.battery\_level \> 10")  
    RecordHighResVideo() \-\> (struct { stream\_handle handle });  
};  
\`\`\`

\`\`\`  
// com.google:maps manifest  
package\_name: "com.google:maps"

sandboxes {  
  // Private encrypted app storage  
  isolated\_storage: true

  // Shared sandbox across mutually trusted ecosystem apps  
  shared\_sandboxes: \[  
    {  
      name: "google\_shared\_vault"  
      scope: "domain://google.com"  \# Enforces PSL/well-known trust check  
      access: READ\_WRITE  
    }  
  \]  
}

resource\_group {  
  // Can join a shared domain cgroup or declare a private pool  
  group\_id: "google\_core\_services"  
  priority: INTERACTIVE  
  cpu\_shares: 1024  
  memory\_limit\_mb: 512  
  gpu\_quota\_percent: 25  
}

processes {  
  name: "maps\_ui"  
  runner: “elf”  
  permissions: \["LOCATION", "STORAGE\_ACCESS"\]  
}  
\`\`\`

### FIDL

To define and govern FIDL services in BexOS across system, ecosystem, and private app boundaries, we can use a unified manifest-driven capability registry managed by the appd.

1\. FIDL Interface Declaration Types

- Strong / System-Level Interfaces: Standardized, OS-defined interfaces (e.g., bexos.hardware.Location, bexos.media.AudioSink). These are baked into the base system distribution and available for any app to request subject to user permission grants.n (CPU time slices, memory high/low watermarks, GPU rendering quotas).  
- Weak / Ecosystem & Vendor Interfaces: Domain-namespaced custom interfaces (e.g., com.google.maps.NavigationService, com.uber.rides.RiderStatus). These can be extended dynamically by third-party developers without modifying the base OS image.

2\. Manifest Service Definition Schema

Services declare their instantiation lifecycle and exposure boundaries in the prototxt manifest:

\`\`\`  
// Manifest for com.google:maps  
package\_name: "com.google:maps"

// 1\. Exposing Services  
services\_exposed: \[  
  {  
    name: "com.google.maps.TileRenderer"  
    protocol: "com.google.maps/TileRendererProtocol"  
      
    // Lifecycle / Multiplicity  
    type: SINGLETON           \# SINGLETON | USER\_SCOPED\_SINGLETON | MULTIPLE\_INSTANCE  
      
    // Visibility / Sharing  
    visibility: SHARED        \# PUBLIC (everyone) | DOMAIN\_SHARED | PRIVATE (sandbox only)  
      
    // Required permission for clients to bind  
    bind\_permission: "com.google.permission.MAP\_RENDER"  
      
    // Key-Value metadata for dynamic query discovery  
    metadata: \[  
      { key: "category", value: "mapping\_v2" },  
      { key: "supports\_offline", value: "true" }  
    \]  
  },  
  {  
    name: "com.google.maps.InternalAuthSync"  
    protocol: "com.google.maps/InternalSyncProtocol"  
    type: USER\_SCOPED\_SINGLETON  
    visibility: DOMAIN\_SHARED  \# Restricted to trusted domains (e.g. google.com, waymo.com)  
  }  
\]

// 2\. Consuming Services (Weak vs Strong Dependencies)  
services\_consumed: \[  
  {  
    // Strong System Dependency (System GPS)  
    name: "bexos.hardware.Location"  
    link\_type: REQUIRED       \# App cannot launch without this service  
  },  
  {  
    // Weak / Ecosystem Dependency  
    name: "com.google.maps.TileRenderer"  
    link\_type: OPTIONAL       \# App dynamically queries; degrades gracefully if absent  
    filter: "supports\_offline \== 'true'"  
  }  
\]  
\`\`\`

| Lifecycle Type | Behavior | Best Used For |
| :---- | :---- | :---- |
| **SINGLETON** | Exactly one global instance runs across the entire system; all clients receive channels to this single server. | Hardware drivers (D0/D1/D2), power management, network stack. |
| **USER\_SCOPED\_SINGLETON** | One instance per authenticated user session/profile. | Credential vaults, notification aggregators, user settings. |
| **MULTIPLE\_INSTANCE** | A new instance or separate worker thread/state is spawned for each client connection. | Media decoders, file parsers, isolated renderer sessions. |

**Visibility ScopeAccess BoundaryEnforcement MechanismPUBLIC**  
Any app installed on the device can discover and bind (if it holds the required permission).

appd checks manifest bind\_permission during channel lookup.

**DOMAIN\_SHARED**  
Only apps within the same eTLD+1 domain or verified in .well-known/bexos-manifest.json can see/bind to it.

appd validates bilateral PSL/well-known origin trust.

**PRIVATE**  
Strictly internal to the declaring package and its child processes.

Unreachable via global directory lookups; invisible to external processes.

**Service Registration:** On launch, a service calls PublishInterface(name, metadata, channel) on its initial handle to the appd.  
**Discovery & Resolution:** A consuming app queries GetInterfaces() with KV metadata or requests GetSingletonInterface("com.google.maps.TileRenderer").  
**Capability Gating:** The appd evaluates the client's manifest against the provider's visibility scope and bind\_permission.  
**Point-to-Point Handoff:** Once verified, appd passes a direct channel endpoint to both parties. Steady-state messaging runs entirely peer-to-peer with zero kernel or broker interception.

### Runners / Life of a process

Runners are managed by the appd and the ELF runner is hardcoded to this. The life of a process is the following:

1. Create the process  
2. Verify the integrity / signature of the runner library and load (if specified)  
   1. compiled as position-independent code (PIC / PIE)  
3. Verify the integrity / signature of the process code and load   
4. Pass the process a FIDL service manager handle and trigger main execution  
5. Mark the process as healthy  
   1. explicit FIDL heartbeat/readiness message from the spawned child (e.g., lifecycle.OnReady()).

There can be many different runners in the future:

- Nix (runs POSIX binaries ala starnix)  
- WASM  
- Web (runs PWA)  
- Android (runs Android app)

Runners can be chained:

- WASM \> JS (QJS) \> React Native

**Runner Policies**

Runner limits are compiled into each board's `platform.pcfg` and decoded by
`appd`. `docs/rfcs/0010/README.md` is the detailed platform policy contract.

To avoid the ISA lock-in that plagued Android’s NDK and JNI ecosystem when attempting architecture transitions (like RISC-V or x86), the boundary must be enforced at the **manifest validation and capability-granting layer** in appd.

If you allow unrestricted native ELF binaries for everyday consumer apps, developers will inevitably optimize exclusively for ARM64 and refuse to compile for future ISAs.

The solution is a **Tiered Runner Policy Model** combining cryptographic trust tiers, manifest declarations, board policy, and an explicit user opt-in mechanism.

### **The Execution & Runner Hierarchy**

| Tier | Allowed Runners | Target Ecosystem | Policy / Verification Requirement |
| :---- | :---- | :---- | :---- |
| **Tier 0: Platform Core** | ELF (Native Rust/C) | Microkernel, Trusty orchestrator, appd   | Signed by OS Vendor key; verified by the authenticated BL33/AVB boot chain.  |
| **Tier 1: System Hardware** | ELF (D1) & WASM (D2) | High-throughput drivers (NVMe, GPU, Audio sink)  | Signed by Silicon/Device Manufacturer; gated by D1/D2 driver permissions.  |
| **Tier 2: Standard Consumer Apps (Default)** | **WASM Only** | All standard consumer apps downloaded from the web / store  | **Strictly WASM.** Any package declaring an ELF runner is rejected by appd unless native ELF is enabled and the package/signer pair is allowlisted in platform policy.  |
| **Tier 3: Verified High-Performance** | Sandboxed WASM \+ Component Model extensions | Games, heavy multimedia editors | Uses WebGPU, SIMD/Threads in WASM, zero-copy buffer handles.  |
| **Tier 4: Developer / Power User (Opt-In)** | ELF, Nix/POSIX, MicroVM | Emulators, developer CLI tools, Linux binaries  | Requires explicit user toggle: **"Enable Native Execution / Developer Mode"**.  |

### **How to Draw and Enforce the Line**

**1\. The Default Rule: Consumer Apps Must Be ISA-Agnostic**

* By default, consumer packages targeting the standard distribution tier are constrained to runner: WASM (or managed web/container runtimes).  
*   
* appd enforces this rule during launch and install validation using `RunnerPolicy`. If a non-system app package declares runner: ELF without a matching policy grant, the request fails immediately:
* 

* *"This package requires native ELF execution, which is restricted on standard consumer profiles."*  
* 

**2\. How to Deliver Native-Grade Performance Inside WASM**  
Developers often reach for native ELF because standard WASM runtimes historically lacked hardware features:

* Expose hardware acceleration directly through capability-backed FIDL channels (WebGPU/Vulkan compute buffers, hardware video decoders, NEON/Vector extensions via WASM SIMD128/256).  
*   
* Provide zero-copy shared memory (SharedArrayBuffer / memory handles) so a WASM game engine can stream vertices to the D1 graphics driver without serialization penalties.  
* 

**3\. Preserving User Power (The Developer Mode Gate)**  
To keep the OS open and respect user agency:

* Include an OS-level switch: **"Allow Unrestricted Native Binaries"** (similar to enabling Developer Options or OEM Unlocking on Android).  
*   
* When toggled, appd permits only the package/signer pairs listed in `native_elf_runner_allowlist` to execute via the ELF runner. Nix, Android, and other legacy runtimes route to the MicroVM policy when enabled.
*   
* This prevents mainstream app stores/commercial developers from demanding native ELF as a baseline requirement, while ensuring hobbyists, power users, and developers can run arbitrary native binaries.  
* 

### **Manifest Enforcement Policy**

In the prototxt manifest, the policy is verified during signature check:

Protocol Buffers  
// Standard App (Accepted automatically)  
package\_name: "com.monzo:app"  
processes {  
  name: "ui"  
  runner: WASM  \# Validated and portable across ARM64, RISC-V, x86\_64  
}

// Native Hardware Driver (Accepted only if signed with System/Vendor Cert)  
package\_name: "com.qualcomm:gpu\_driver"  
processes {  
  name: "adreno\_d1"  
  runner: ELF   \# Verified against Vendor Hardware Trust Anchor  
  service { phase: DRIVER }  
}

This ensures that the consumer ecosystem remains 100% architecture-independent, allowing BexOS to switch or add CPU architectures (like RISC-V) seamlessly without breaking a single consumer application.

### File Systems

**No, you should not implement a global Linux-style VFS root (**/**).**

A global root filesystem creates **ambient authority**—where any process can attempt to access /etc/shadow, /dev, or /data simply by constructing a path string. It also creates shared, mutable global state that complicates capability security, sandboxing, and heart transplant state snapshots.

Instead, BexOS should use a **Capability-Based Per-Process Namespace Model** (similar to Fuchsia's namespaces, Plan 9, and WASI dirfd capabilities).

### **How Filesystem Capabilities Work Without a Global Root**

In this architecture, filesystems are unprivileged userspace services, and a "path" only exists relative to a capability handle that a process explicitly holds.

* \+-------------------------------------------------------------------------+  
* |                  Storage Services (Userspace D1 / VFS)                  |  
* |                                                                         |  
* |  \+---------------------------+       \+-------------------------------+  |  
* |  | NVMe / Block Driver (D1)  | \<---\> | BexFS / Ext4 Filesystem Server|  |  
* |  \+---------------------------+       \+-------------------------------+  |  
* \+-------------------------------------------------------------------------+  
*                                      |  
*                                      | Mints Root Directory Capabilities  
*                                      v  
* \+-------------------------------------------------------------------------+  
* |                  appd (Namespace Assembler)                      |  
* |                                                                         |  
* |  1\. Holds Storage Service capabilities                                  |  
* |  2\. Creates isolated app storage sub-trees                              |  
* |  3\. Assembles custom virtual namespaces for each spawned process        |  
* \+-------------------------------------------------------------------------+  
*                                      |  
*          \+---------------------------+---------------------------+  
*          |                                                       |  
*          v                                                       v  
* \+----------------------------------+   \+----------------------------------+  
* | Process A Namespace:             |   | Process B Namespace:             |  
* |  /pkg    \-\> \[Read-Only Bundle\]   |   |  /pkg    \-\> \[Read-Only Bundle\]   |  
* |  /data   \-\> \[Private App Encrypt\]|   |  /data   \-\> \[Private App Encrypt\]|  
* |  /shared \-\> \[Google Shared Vault\]|   |  /tmp    \-\> \[In-Memory MemFS\]    |  
* \+----------------------------------+   \+----------------------------------+

### **Core Filesystem Architecture**

**1\. Filesystems as FIDL Services**  
Filesystem implementations (e.g., a native Rust BexFS, a FAT32 driver, or an in-memory MemFS) run as userspace services. They expose a standardized FIDL interface:

Code snippet

* library bexos.fs;  
*   
* protocol Node {  
*     GetAttr() \-\> (struct { attributes FileAttributes });  
*     Close();  
* };  
*   
* protocol File {  
*     compose Node;  
*     Read(struct { count uint64 }) \-\> (struct { data vector\<uint8\> });  
*     Write(struct { data vector\<uint8\> }) \-\> (struct { actual uint64 });  
*     Seek(struct { offset int64, whence SeekOrigin }) \-\> (struct { new\_offset uint64 });  
*     GetBuffer() \-\> (resource struct { buffer handle:VMO }); // Zero-copy mmap  
* };  
*   
* protocol Directory {  
*     compose Node;  
*     Open(struct {  
*         flags OpenFlags,  
*         path string,  
*         object server\_end:Node  
*     });  
*     ReadEntries() \-\> (struct { entries vector\<DirEntry\> });  
* };

**2\. Assembling Per-Process Namespaces**  
When appd launches a process, it builds a local **Namespace Table** (a lightweight routing dictionary) mapping virtual path prefixes directly to channel capabilities:

| Virtual Mount Path | Backing Capability Source | Permissions |
| :---- | :---- | :---- |
| /pkg | Read-only bundle partition handle for this .bexapp | Read / Execute |
| /data | Private encrypted storage directory handle | Read / Write |
| /shared/<vault name> | Domain/local shared directory handle (e.g., google\_shared\_vault) | Read / Write (if manifest and peer trust pass) |
| /tmp | Ephemeral MemFS directory handle created on launch | Read / Write |
| /svc | Channel to appd for dynamic FIDL discovery | Connect / Bind |

**3\. Built-In Sandboxing (No Path Traversal Escapes)**  
Because path resolution starts from an explicit Directory handle:

* A process executing open("/data/../../etc/passwd") cannot escape /data.  
*   
* When the filesystem server resolves paths relative to the /data directory handle, attempting to step above the granted root node returns ZX\_ERR\_ACCESS\_DENIED or resolves to /data itself.  
*   
* There is no /etc, /root, or /sys unless appd explicitly inserts those handles into the process namespace.  
* 

### **How** appd **Manages Storage**

* **Bootstrapping Storage:** During boot, appd connects to the raw storage service (which mounts disk partitions).  
*   
* **Directory Provisioning:** When an app is installed, appd asks the filesystem service to create a cryptographically isolated directory node (e.g., fs.CreateDirectory("apps/com.monzo:app/data")).  
*   
* **Sub-Directory Delegation:** When launching com.monzo:app, appd passes only the channel handle representing that specific sub-tree to the child process as /data.  
* 

### **Handling POSIX Compatibility & WASI**

* **Embedded POSIX Shims (**libposix **/** musl**):** When legacy C/C++ or Nix runner code calls open("/pkg/assets/logo.png", O\_RDONLY), the userspace shim intercepts the call, finds the matching /pkg channel handle in the process namespace, and sends a Directory.Open("assets/logo.png") FIDL request.  
*   
* **WASI Compatibility:** WASI natively uses capability-based file descriptors (dirfd). Mapping WASI fd\_read / path\_open onto BexOS directory handles requires near-zero translation overhead.  
*   
* **Heart Transplants:** Because filesystem state consists of explicit open channel endpoints rather than complex kernel-level VFS lock trees and inode caches, serializing and reconnecting storage handles across a microkernel swap is completely deterministic.

**Kernel FIDL**

**Because BexOS follows a strict capability microkernel design, the kernel exposes no ambient POSIX-like system calls. Instead, it exposes a minimal set of foundational object-capability interfaces over primitive syscall channels/trap boundaries.**

**The kernel handles are divided into four essential capability primitives: Channels (IPC), Memory Objects (VMOs), Task/Execution Management (Threads/Futexes), and Clocks/Timers.**

### idl/bexos/kernel/types.fidl

**Code snippet**  
library bexos.kernel;

/// Universal capability handle rights bitmask

type Rights \= strict bits : uint32 {

    TRANSFER        \= 0x00000001; // Can be transferred across channels

    READ            \= 0x00000002; // Can read bytes / messages

    WRITE           \= 0x00000004; // Can write bytes / messages

    EXECUTE         \= 0x00000008; // Executable memory mapping (W^X restricted)

    MAP             \= 0x00000010; // Can be mapped into virtual address space

    DUPLICATE       \= 0x00000020; // Can duplicate handle with equal/fewer rights

    SIGNAL          \= 0x00000040; // Can trigger/wait on object signals

    MANAGE\_TASK     \= 0x00000080; // Suspend/Resume/Kill tasks

    ADMIN           \= 0x80000000; // Reserved for app\_service & Orchestrator

};

/// Signals emitted on handles for asynchronous polling/waiting

type Signals \= strict bits : uint32 {

    READABLE        \= 0x00000001;

    WRITABLE        \= 0x00000002;

    PEER\_CLOSED     \= 0x00000004;

    SIGNALED        \= 0x00000008;

    SUSPENDED       \= 0x00000010;

    TERMINATED      \= 0x00000020;

};

type Status \= strict enum : int32 {

    OK                      \= 0;

    ERR\_INVALID\_HANDLE      \= \-1;

    ERR\_ACCESS\_DENIED       \= \-2;

    ERR\_NO\_MEMORY           \= \-3;

    ERR\_BUFFER\_TOO\_SMALL    \= \-4;

    ERR\_PEER\_CLOSED         \= \-5;

    ERR\_TIMED\_OUT           \= \-6;

    ERR\_ALREADY\_EXISTS      \= \-7;

    ERR\_INVALID\_ARGS        \= \-8;

};

### idl/bexos/kernel/ipc.fidl

**Code snippet**  
library bexos.kernel;

/// Core bi-directional point-to-point IPC channel primitive

protocol ChannelControl {

    /// Allocate a paired bi-directional channel endpoint

    CreateChannel() \-\> (resource struct {

        status Status,

        local\_endpoint handle:CHANNEL,

        remote\_endpoint handle:CHANNEL

    });

    /// Read message bytes and transferred capability handles

    ReadMessage(resource struct {

        channel handle:CHANNEL,

        max\_bytes uint32,

        max\_handles uint32

    }) \-\> (resource struct {

        status Status,

        data vector\<uint8\>,

        handles vector\<handle\>

    });

    /// Write message bytes and transfer capability handles to the remote peer

    WriteMessage(resource struct {

        channel handle:CHANNEL,

        data vector\<uint8\>,

        handles vector\<handle\>

    }) \-\> (struct {

        status Status

    });

};

### idl/bexos/kernel/memory.fidl

**Code snippet**  
library bexos.kernel;

type VmoFlags \= strict bits : uint32 {

    RESIZABLE       \= 0x00000001;

    CONTIGUOUS\_PHYS \= 0x00000002; // For D1 DMA drivers

    CACHE\_POLICY\_WB \= 0x00000004; // Write-Back Cache

    CACHE\_POLICY\_UC \= 0x00000008; // Uncached / MMIO registers

};

/// Virtual Memory Object (VMO) manipulation for zero-copy allocations & sharing

protocol VirtualMemory {

    /// Create a page-aligned anonymous VMO

    CreateVmo(struct {

        size\_bytes uint64,

        flags VmoFlags

    }) \-\> (resource struct {

        status Status,

        vmo handle:VMO

    });

    /// Map a VMO range directly into the calling process address space

    Map(resource struct {

        vmo handle:VMO,

        vmo\_offset uint64,

        size\_bytes uint64,

        target\_vaddr uint64, // 0 for kernel-selected address

        requested\_rights Rights

    }) \-\> (struct {

        status Status,

        mapped\_vaddr uint64

    });

    /// Unmap a virtual memory region

    Unmap(struct {

        vaddr uint64,

        size\_bytes uint64

    }) \-\> (struct {

        status Status

    });

    /// Create a Copy-on-Write (CoW) clone of an existing VMO

    CloneVmo(resource struct {

        parent\_vmo handle:VMO,

        offset uint64,

        size\_bytes uint64

    }) \-\> (resource struct {

        status Status,

        cloned\_vmo handle:VMO

    });

};

### idl/bexos/kernel/task.fidl

**Code snippet**  
library bexos.kernel;

/// Thread execution and lightweight synchronization (Futex)

protocol TaskControl {

    /// Spawn an execution thread within the current address space

    CreateThread(resource struct {

        entry\_vaddr uint64,

        stack\_top\_vaddr uint64,

        arg\_handle handle:OPTIONAL

    }) \-\> (resource struct {

        status Status,

        thread\_handle handle:THREAD

    });

    /// Terminate calling thread

    ExitThread(struct { exit\_code int32 });

    /// Fast userspace mutual exclusion wait (futex)

    FutexWait(struct {

        uaddr uint64,

        expected\_val uint32,

        timeout\_nanos int64

    }) \-\> (struct {

        status Status

    });

    /// Wake threads sleeping on a futex address

    FutexWake(struct {

        uaddr uint64,

        wake\_count uint32

    }) \-\> (struct {

        status Status,

        woken\_count uint32

    });

    /// Wait on signals across multiple handles simultaneously

    WaitMany(resource struct {

        items vector\<struct {

            h handle,

            signals Signals

        }\>,

        deadline\_nanos int64

    }) \-\> (struct {

        status Status,

        satisfied\_index uint32,

        observed\_signals Signals

    });

};

### idl/bexos/kernel/system.fidl **(Restricted:** app\_service **/ Drivers Only)**

**Code snippet**  
library bexos.kernel;

/// Privileged capability operations gated strictly to app\_service and D0/D1 drivers

protocol SystemPrivileged {

    /// Create a new isolated process container and address space

    CreateProcess(struct {

        name string:64,

        resource\_group\_id uint32

    }) \-\> (resource struct {

        status Status,

        process\_handle handle:PROCESS,

        address\_space\_handle handle:VM\_SPACE

    });

    /// Bind a hardware interrupt vector to an event handle (D0/D1 drivers)

    BindInterrupt(struct {

        irq\_number uint32,

        flags uint32

    }) \-\> (resource struct {

        status Status,

        irq\_handle handle:INTERRUPT

    });

    /// Heart Transplant: Freeze address spaces and serialize capability graph

    CheckpointSystemState(resource struct {

        target\_vmo handle:VMO

    }) \-\> (struct {

        status Status,

        serialized\_bytes uint64

    });

};

### **Summary of the Abstraction**

| Interface | Exposed To | Purpose |
| :---- | :---- | :---- |
| ChannelControl | **All Apps & Services** | **Zero-copy point-to-point IPC and capability transfers.** |
| VirtualMemory | **All Apps & Services** | **VMO allocation, page-table mappings, and shared memory handles.** |
| TaskControl | **All Apps & Services** | **Multi-threading, Futex synchronization, and event polling.** |
| SystemPrivileged | app\_service **& D1 Drivers** | **Process creation, IRQ bindings, and Heart Transplant checkpoint serialization.** |

**Standard apps use only the unprivileged primitives, interacting with devices and OS features exclusively by exchanging FIDL messages over channels brokered by the userspace** appd**.**
