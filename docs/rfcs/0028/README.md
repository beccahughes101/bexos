# RFC 0028: Shared libraries and package dependencies

- Created: 2026-08-30T08:02:50-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

Native libraries use function calls and versioned ABIs; WASM libraries use component interfaces. Signed library packages share the package resolver and private dependency namespace.

## Design overview

In-process libraries should use function calls or native interfaces rather than FIDL IPC.

FIDL is optimized for cross-process boundary serialization, kernel handle validation, and IPC channels. Using FIDL inside a single process adds unnecessary serialization overhead, message encoding, and buffer allocations where standard function calls or native FFI achieve near-zero latency.

## The In-Process Library Strategy

Libraries in BexOS fall into three execution models based on language, safety, and performance requirements:

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ APPLICATION PROCESS (WASM Container or Native Runtime)                      │
│                                                                             │
│  [ High-Performance Pure Native Libraries ]                                 │
│    • `bexos-crypto` (BLAKE3, ChaCha20, Ed25519)                             │
│    • Linked statically as Rust crates or dynamically as position-independent │
│      ELF shared objects (`.so`)                                             │
│    • Communication: Direct C-ABI / Rust native function calls (~0ns)        │
│                                                                             │
│  [ WASM Native Component Libraries ]                                        │
│    • Utility modules, format parsers, business logic compiled to WASM       │
│    • Communication: WebAssembly Component Model (Wasm Canonical ABI / WIT)  │
│    • Bound directly in the Wasm engine without memory copies                │
│                                                                             │
│  [ Framework Native Bridges (e.g., React Native / QuickJS / Flutter) ]      │
│    • JS/Wasm UI bindings to device capabilities                             │
│    • Communication: Direct Host Function Imports (Host-Defined Hostcalls)   │
│      or In-Memory ArrayBuffer shared slices                                 │
│                                                                             │
│  [ SYSTEM IPC BOUNDARY ]                                                    │
│    ▼ Only when crossing process boundaries to appd / vfsd / netstack / teed │
│    Use FIDL over bexos::kernel::Channel                                     │
└─────────────────────────────────────────────────────────────────────────────┘

```

## High-Performance Native Libraries (e.g., Crypto, Math)

For native libraries requiring maximum raw CPU speed (like `bexos-crypto` or image decoders):

* **Mechanism:** Direct **C ABI / Rust static linking** or dynamic `.so` loading.
* **Why Not FIDL?** Hashing a 100MB buffer via BLAKE3 takes ~10–20ms natively by passing a raw memory pointer `(&[u8])`. Serializing that buffer into a FIDL struct or copying it through an IPC ring would introduce gigabytes/sec of memory copying and CPU cache invalidation.

### Usage

```rust
// In-process direct call: zero allocation, zero copies
let hash = bexos_crypto::blake3::hash(&buffer[..]);

```


## WASM Libraries (WebAssembly Component Model & WIT)

For modular libraries written in multiple languages (Rust, Go, C++, Zig) loaded into a WASM app:

* **Mechanism:** **WebAssembly Interface Types (WIT) + Canonical ABI** (Wasm Component Model).
* **Why WIT over FIDL?** WIT is the open, standardized interface definition language built specifically for in-process Wasm boundaries. The Wasm engine can inline function calls and pass flat scalar records and linear memory pointers directly between modules without serializing bytes.

### Interface Definition (`crypto.wit` / `parser.wit`)

```wit
package bexos:parser;

interface json-validator {
    record ValidationResult {
        valid: bool,
        error-offset: u32,
    }
    validate: func(payload: list<u8>) -> ValidationResult;
}

```


## Framework Native Bridges (React Native, QuickJS, Flutter)

When a JavaScript or UI framework running inside the app process needs access to native capabilities or native libraries:

* **Mechanism:** **Hostcall Imports / Host Functions** registered with the embedded runtime (QuickJS / Hermes / Wasmtime).
* **Data Transport:** Shared `ArrayBuffer` or `Uint8Array` views on memory.

### Flow

1. The JS/Wasm app calls `globalThis.__bexos_native_draw_rect(buffer.byteOffset, width, height)`.
2. The host runtime executes the native C/Rust function immediately on the current thread without message queues or marshalling layers.

## When to Use What

| Boundary | Technology | Calling Convention | Latency |
| --- | --- | --- | --- |
| **In-Process Native Lib** | Native Rust / C ABI | Direct Function Call (`call / bl`) | $< 1\text{ ns}$ |
| **In-Process WASM Module** | Wasm Component Model (WIT) | Wasm Canonical ABI Call | $5\text{–}15\text{ ns}$ |
| **In-Process JS/Native Bridge** | QuickJS / Hermes Host Functions | Direct C Function Binding | $20\text{–}50\text{ ns}$ |
| **Cross-Process Service** | **FIDL** (`appd`, `vfsd`, `netstack`, `teed`) | Kernel Channels / IPC Messages | $0.5\text{–}2.0\text{ µs}$ |

## Design Guideline for BexOS SDK

* Use **FIDL** exclusively as the **Inter-Process Boundary Definition** (IPC between `appd`, `vfsd`, `netstack`, `teed`, and app processes).
* Use **Rust crates and C-ABI headers** for native static/shared in-process libraries.
* Use **WIT (Wasm Interface Types)** for modular, multi-language in-process WASM libraries.

Shared libraries use standard `.bex` packages mounted under `/deps/<package_name>` in the application's private namespace.

This keeps packaging uniform and uses the existing content-addressed storage and Zstd demand-paging pipelines without introducing another container format.

Current implementation status: package manifests distinguish applications from
non-launchable library packages and can declare library dependencies.
`bexos.lib.crypto` builds a freestanding AArch64 ELF shared object at
`lib/libbexos_crypto.so`, is packaged as a signed library package, and is also
embedded in BootFS for early platform services. appd maps declared crypto
library dependencies into process VM, applies the AArch64 relocations emitted by
the current Rust shared-object build, resolves the exported versioned C ABI, and
passes a loader link map to `//lib/crypto_client`. Dependency resolution now
uses the app registry, active pins, and partial package/version selectors before
opening the selected archive through vfsd. Guest keychain, users, and
debugd binaries now depend on `//lib/crypto_client` instead of statically
including `//lib/crypto`; host tools and host tests continue to use the direct
Rust crate.

## Manifest Schema: Package Kinds & Dependency Declarations

Differentiate between standalone applications and shared libraries using a `package_kind` field in the manifest:

```protobuf
syntax = "proto3";

package bexos.manifest;

enum PackageKind {
  APPLICATION = 0; // Contains an executable entrypoint/runner
  LIBRARY     = 1; // Pure asset/code bundle (shared objects, Wasm modules)
}

message LibraryDependency {
  // Target library package name (e.g. "com.bexos.lib.react_native")
  string package_name = 1;

  // Semantic version constraint or exact BLAKE3 content hash
  string version_requirement = 2;

  // Optional custom mount alias (defaults to package_name)
  string mount_alias = 3;

  // Required runtime ABI version exported by the selected library package
  uint32 abi_version = 4;
}

message PackageManifest {
  string package_id = 1;
  PackageKind kind = 2;
  bexos.version.SemVer package_version = 3;
  bexos.version.MultiVersionPolicy multi_version_policy = 4;

  // Library dependencies required by this application/module
  repeated LibraryDependency dependencies = 5;
}

```

## How `appd` and `vfsd` Mount the Dependency Tree

When `appd` spawns an application process, it constructs an isolated VFS namespace with direct capability handles:

```
[ Process Namespace Layout ]
/pkg/                             ──► Root of the app's own .bex container
/data/                            ──► App's private encrypted vault storage (/vault/user_1000/...)
/deps/
  ├── com.bexos.lib.react_native/ ──► Read-only ArchiveFS mount of React Native .bex
  │     ├── lib/react_native.wasm
  │     └── assets/
  └── com.bexos.lib.crypto/       ──► Read-only ArchiveFS mount of Crypto .bex
        └── lib/libbexos_crypto.so

```

```
[ Application Launch Flow ]
1. `appd` resolves the dependency graph from the app's `manifest.proto`,
   accepting either `package_name: "com.bexos.lib.react_native:v0.74"` or
   `package_name: "com.bexos.lib.react_native"` with
   `version_requirement: "v0.74"`.
2. For each dependency, `appd` verifies the signature and grabs the `.bex` from `/system/packages/`.
3. `vfsd` mounts each library container as an immutable, read-only `ArchiveFS` node.
4. `appd` injects these directory handles into the process namespace at `/deps/<package_name>`.
5. The runtime loader (dynamic linker or Wasm Component engine) loads modules from `/deps/<package_name>/lib/...`.

```

## Advantages of the `/deps` Namespace Model

* **Uniform Packaging & Verification:** Libraries use the identical Ed25519 + BLAKE3 + Zstd packaging pipeline as apps, avoiding redundant tooling or formats.
* **Hermetic & Zero Side-Effects:** No global `/usr/lib` or DLL search paths. A library is visible to a process *only* if explicitly declared in its manifest.
* **Safe Side-by-Side Versioning:** Two apps can run simultaneously using different versions of the same shared library without collision:
* App A mounts `com.bexos.lib.react_native:v0.74` to `/deps/com.bexos.lib.react_native`
* App B mounts `com.bexos.lib.react_native:v0.76` to `/deps/com.bexos.lib.react_native`

Partial version selectors are prefix matches against structured SemVer labels.
If a selector matches more than one installed version, resolution fails instead
of picking an arbitrary package.

* **Zero-Copy Memory Deduplication:** If multiple apps mount the same library version, `vfsd` shares the underlying read-only physical memory frames (VMOs) across all address spaces.
