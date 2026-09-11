# RFC 0020: In-process WASM sandboxes

- Created: 2026-08-28T11:49:15-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

Applications can host child WASM sandboxes with bounded resources and signature-gated capability delegation. Engine checks and validated host interfaces enforce isolation; QuickJS integration and richer delegated interfaces remain future design.

## Implementation status

The runtime implementation lives in `lib/wasm_runtime`, the launch ABI in
`lib/wasm_abi`, and the native embedding in `services/wasm_runner`. See
[the current runtime page](../../wasm-runtime.md) for supported interfaces
and verification status. Implemented child controls include spawn, invoke,
status, pause, resume, and termination. Signature validation limits delegation to
parent resources; unsigned children receive bounded handle-free messaging. Live
paused children participate in service checkpoint migration, including rollback.
The QuickJS compiler, JS packaging, graphics/input
allowlist, and driver examples below remain future design.

The in-process boundary relies on WebAssembly validation, engine checks, and
checked host interfaces. Owned VMARs organize mappings and guard pages; separate
VMARs in one process do not create a hardware security boundary. The trusted
runner and engine remain part of the application’s trusted computing base.

By combining **in-process sub-sandboxes** with **QuickJS bytecode packaging (similar to Shopify’s Javy / Wasm Component Model)**, a host app can run untrusted third-party JavaScript within a checked WebAssembly boundary, provided the engine and host interfaces enforce the restrictions described here.

## System Architecture Overview

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ HOST WASM RUNTIME PROCESS (Main App: e.g. Notion, VSCode, Browser UI)      │
│                                                                             │
│  [ Main App Logic ]                                                         │
│     │                                                                       │
│     ├── 1. Generates/Fetches Untrusted JS Plugin Source                     │
│     ├── 2. Emits Self-Contained Wasm Module (Javy-style: QJS Bytecode + VM) │
│     ▼                                                                       │
│  [ In-Process Sandbox Host (`bexos:wasm/sandbox`) ]                         │
│     │                                                                       │
│     ├─ Allocates Sub-VMAR (Private Linear Memory: 0x20000000..0x20040000)  │
│     ├─ Instantiates Child Wasm Engine Instance                              │
│     └─ Links Restricted In-Process FIDL Channel Endpoints                   │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │
                                       ▼ Validated WASM / Checked Host Boundary
┌─────────────────────────────────────────────────────────────────────────────┐
│ IN-PROCESS WASM SANDBOX (Untrusted Plugin / Extension)                      │
│                                                                             │
│  ┌───────────────────────────────────────────────────────────────────────┐  │
│  │ Embedded QuickJS Interpreter Engine                                   │  │
│  │  ▲                                                                    │  │
│  │  └── Evaluates Pre-compiled JS Bytecode in Private Linear Heap        │  │
│  └───────────────────────────────────┬───────────────────────────────────┘  │
│                                      │                                      │
│  [ Restricted Sub-Interface FIDL ] ◄─┘ (Can ONLY talk to granted endpoints) │
│    • Has access to: `PluginApi.SetStatusText()`, `PluginApi.GetDocText()`   │
│    • NO access to: Camera, Raw Sockets, Filesystem Root, Host RAM           │
└─────────────────────────────────────────────────────────────────────────────┘

```

## Future: How JS Compiles to Runnable Wasm (The Javy Pattern)

QuickJS (`qjsc`) emits **bytecode**, which requires an interpreter loop to evaluate. To make this an atomic Wasm module:

### The Plugin Compiler (In Host)

* The host takes the JS source string and passes it to the QuickJS bytecode emitter (`JS_WriteObject`).
* It packages that bytecode array into the `.data` section of a pre-compiled, minimal QuickJS interpreter Wasm binary (the **Javy** approach).

2. **Result (future):** A self-contained `.wasm` plugin module. Artifact size and startup cost require measurement with the selected QuickJS build.

## The In-Process Sandbox Host API (`bexos:wasm/sandbox`)

The host application uses a native WASM host import to spin up isolated sandbox instances:

```fidl
library bexos.wasm.sandbox;

using bexos.kernel;

type SandboxConfig = struct {
    max_memory_pages uint32;       // Maximum 64KB Wasm memory pages (e.g., 64 = 4MB)
    max_instruction_fuel uint64;   // Fuel metering to prevent infinite loops
    allow_threads bool;
};

@discoverable
protocol InProcessSandboxManager {
    /// Instantiate a child Wasm module in a private sub-VMAR within this process
    SpawnSandbox(resource struct {
        wasm_bytes handle:VMO,     // Compiled QuickJS plugin module
        config SandboxConfig,
        granted_channel handle:CHANNEL // The single FIDL channel passed to the child
    }) -> (resource struct {
        status bexos.kernel.Status,
        sandbox_control handle:CHANNEL // Host handle to pause/resume/kill sandbox
    });
};

```

## Expected properties and validation requirements

### A. Capability Attenuation (Sub-Slicing Permissions)

* The main application might hold top-level capabilities for `/data/user_documents` and `bexos.net.TcpSocket`.
* When spawning the plugin sandbox, the host **does not pass those capabilities**.
* It only passes a handle to a custom sub-interface (e.g., `MarkdownPluginProtocol`). The sandbox cannot address, sniff, or invoke any host capability it was not explicitly passed.

### B. Zero-Process Context Switch Overhead

* Spawning a full OS process for a 50-line plugin adds process table bloat and CPU context-switch latency.
* **In-process Wasm sandboxes execute within the same address space** (bounded by WASM validation and checked host interfaces; Sub-VMARs organize the mappings). An in-process invocation avoids creating a process. Kernel channel operations still cross the kernel boundary. Compilation, invocation, copying, and scheduling costs require BexOS measurements; no latency guarantee is established.

### C. Deterministic Fuel Metering (Anti-Denial of Service)

* Untrusted JavaScript containing `while(true) {}` cannot freeze the host UI.
* The Wasm runtime injects instruction counting (fuel). When fuel reaches zero, the host catches the trap, terminates the sandbox instance via `sandbox_control`, and notifies the user: *"Plugin 'SpellChecker' exceeded CPU limit."*

## Summary of the Flow

1. User installs or runs an untrusted JS extension (e.g., in a BexOS markdown editor).
2. Host app uses embedded QuickJS compiler to turn JS into a bytecode-backed Wasm module.
3. Host calls `SpawnSandbox()`, allocating a Sub-VMAR for 4MB of private WASM linear memory and passing an attenuated FIDL channel.
4. Plugin executes in Wasm at high speed, communicates only over the granted FIDL channel, and cannot escape its sandbox memory or freeze the host app.

The signature policy distinguishes verified code from unsigned code without trusting a guest-provided origin label.

The policy lets applications such as React Native, Flutter, and Figma load pre-packaged code with rich capabilities while restricting untrusted downloaded scripts and generated bytecode from obtaining full host access.

## The Two-Tier Attenuation Model

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ Tier A: Signed Child Bundle (e.g. React Native `index.android.bundle.bex`)  │
├─────────────────────────────────────────────────────────────────────────────┤
│ • Verified by TEE / Appd signature trust root                               │
│ • Host can delegate: ANY SUBSET of host's own granted capabilities         │
│   (e.g., Camera, Bluetooth, Network Sockets, Private Storage /data)         │
└─────────────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────────────┐
│ Tier B: Unsigned / JIT-Generated Code (Dynamic JS Plugins / Eval)          │
├─────────────────────────────────────────────────────────────────────────────┤
│ • Generated in-process (e.g., QuickJS QBC via `qjsc` or dynamic JIT)        │
│ • Host can delegate: ONLY a strict Platform-Defined Safe Allowlist          │
│   (e.g., Canvas/2D draw surfaces, pure compute channels, in-memory buffers)│
│ • RESTRICTED BY DEFAULT: No direct sockets, no camera, no raw VFS roots     │
└─────────────────────────────────────────────────────────────────────────────┘

```

## How the Attenuation Boundary is Enforced

When the main application calls the in-process sandbox manager (`bexos.wasm.sandbox.SpawnSandbox`), the runtime checks the cryptographic status of the module payload before slicing capabilities:

```
[ Host Application ]
       │
       ├─ Calls SpawnSandbox(wasm_vmo, granted_channels)
       │
       ▼
┌────────────────────────────────────────────────────────────────────────┐
│ Wasm Sandbox Host Runtime (`wasm_runner`)                              │
│                                                                        │
│ 1. Inspects `wasm_vmo` signature:                                      │
│    ├── IF Valid Signature (Signed by Verified Developer/Vendor):      │
│    │     ► Grants requested capabilities (provided host already holds   │
│    │       them).                                                      │
│    │                                                                   │
│    └── IF Unsigned (or Local QJS JIT Artifact):                        │
│          ► Computes CapSet = Granted ∩ Strict_Unsigned_Allowlist       │
│          ► If any non-allowlisted channel is passed:                   │
│            REJECTS spawn with `ERR_UNAUTHORIZED_CAPABILITY_DELEGATION` │
└────────────────────────────────────────────────────────────────────────┘

```

## The Strict Allowlist for Unsigned Code

The implemented admission policy allows bounded, handle-free parent messaging
and private computation resources. Graphics and input entries below are future
proposals, not currently delegable capabilities. Unsigned children cannot receive
external handles at spawn or through subsequent transfers. Invalid signatures
reject admission and never downgrade to unsigned execution.

Unsigned dynamic code is constrained to safe, non-exfiltrating primitives:

### Safe Allowlisted Capabilities

* In-memory UI buffer rendering / Canvas surfaces (`bexos.graphics.Surface`).
* Structured input events (sanitized mouse/touch coordinates).
* Fuel / execution limits (CPU quota & memory high-watermarks).
* Bounded parent messaging without native channel handles.

### Blocked Capabilities (Forbidden for Unsigned Code)

* Arbitrary outbound network sockets (`bexos.net.TcpSocket`).
* Persistent storage handles (`bexos.fs.Directory` / `/data`).
* Sensor / Hardware hardware registers (Camera, Microphone, GPS).
* Task / Thread creation outside the bounded sub-VMAR.

## FIDL Interface with Cryptographic Verification

```fidl
library bexos.wasm.sandbox;

using bexos.kernel;

type CodeOrigin = strict enum : uint8 {
    VERIFIED_SIGNATURE = 1;
    UNSIGNED_GENERATED = 2;
};

protocol InProcessSandboxManager {
    /// Instantiate a sandbox with signature-gated capability attenuation
    SpawnSandbox(resource struct {
        wasm_bytes handle:VMO,
        signature_block vector<uint8>:512?, // Optional PKCS#7 / Ed25519 signature
        granted_handles vector<handle>:16   // Capability endpoints to attenuate
    }) -> (resource struct {
        status bexos.kernel.Status,
        origin CodeOrigin,                  // Reports verified tier
        sandbox_control handle:CHANNEL
    });
};

```

## Design properties

1. **First-Class React Native & Dynamic Frameworks:** Official, signed JS bundles bundled with the app can access camera, storage, and networking without restriction.
2. **Safe Third-Party Plugin Ecosystems:** When a user installs an untrusted community plugin inside Notion or VS Code, the host can run it without fear of data exfiltration or silent background network telemetry.
3. **No Ambient Escalation:** Even if a compromised host app attempts to grant its raw network channel to an unsigned JIT payload, the kernel/runner halts the delegation because the target code lacks a valid signature token.
