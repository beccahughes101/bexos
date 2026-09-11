This is the repo for BexOS an experimental highly modular OS written in rust.

**This is a hobby project experiment, do not use**

This project aims to answer the question, if we were to build an OS in 2026, what would it look like. We live in a world where creativity is accelerated thanks to AI, but current platforms and operating systems are holding back progress. What if you could have a platform where people can choose which components they want to use, and have the freedom to make those choices securely. Updates are always problematic, no-one wants to restart their devices, with our super modular OS we can update each component individually. Our heart transplant technology allows state syncing so updates are seamless and resume state less than 150 ms. This applies to everything, including the kernel itself, we don't use patches because that would break the signature of the kernel, we start the new version and sync the data between them.

### Goals

- Secure
  - Support for trusty + TEEs as a native requirement
  - Everything is signed  
  - Allowlist permission
  - Most apps will use WASM
- Designed for the modern world  
  - i.e. designed for multi core systems & AI
  - Community driven change process (RFCs, future)
  - Modern views framework so that all BexOS systems and apps are visually appealing (future)
- Highly performant and robust  
  - Rust first
  - Everything is components and hot swappable (all D1 drivers + services should support heart transplant)
- Open source
- Extendable (e.g. chrome like extensions in any app or the OS itself)
  -  A end user (non technical) should be able to use an AI agent to create an app on the fly

Technology choices:

- Rust  
- Go (where needed, probably server)  
- Connect RPC (client to server using quic)  
- Protobuf (prototxt to compiled buffer)  
- Bazel
- redb for storage

### Repo Structure

```text
bexos/
├── MODULE.bazel                 # Bzlmod dependencies (rules_rust, rules_cc, etc.)
├── .bazelrc                     # Multi-target platform flags & rustc settings
├── build/
│   ├── platforms/               # Platform definitions (aarch64_none, wasm32)
│   ├── toolchains/              # Cross-compilers (LLVM, Rust, WASM)
│   └── rules/                   # Starlark macros (.fidl codegen, .bexapp packaging)
│
├── docs/                         # Always document 
├── idl/                         # Central FIDL Interface Definitions
│   ├── bexos/
│   │   ├── hardware/            # bexos.hardware.Location, Camera, etc.
│   │   ├── system/              # bexos.system.Appd, Orchestrator
│   │   └── media/               # bexos.media.AudioSink, WebGPU
│   └── BUILD.bazel
│
├── tools/                       # Host-Side Build & Code Generation Tools
│   ├── fidlc/                   # Forked FIDL compiler (+@permission & CEL AST split)
│   ├── qemu/                    # Bazel wrappers for QEMU bring-up
│   ├── packager/                # .bexapp bundler & manifest compiler (prototxt -> bin)
│   └── cert-tools/              # VMC/BIMI brand verifier & signing utilities
│
├── secure/                      # Secure-world models and Trusty app designs
│   ├── orchestrator/            # Root-of-Trust supervisor, A/B fallback, watchdog
│
├── kernel/                      # Non-Secure EL1 Microkernel (no_std Rust)
│   ├── core/                    # Testable no_std allocator, MMU, scheduler, IPC logic
│   ├── src/
│   │   ├── arch/aarch64/        # MMU 4-level paging, GIC, context switching
│   │   ├── memory/              # Physical frame allocator, grant tables
│   │   ├── ipc/                 # Channel endpoints & zero-copy buffer handoff
│   │   ├── sched/               # Preemptive scheduler & resource group (cgroup) hooks
│   │   └── criu/                # Heart-transplant freeze/restore dispatch
│   └── BUILD.bazel
│
├── services/                    # Userspace System Servers (Native Rust)
│   ├── appd/             # init, manifest manager, CEL evaluator, broker
│   ├── vmm/                     # MicroVM monitor (Linux/Android guest isolation)
│   ├── compositor/              # UI surface manager & WebGPU pipeline
│   └── vfs/                     # Capability-based encrypted storage broker
│
├── drivers/                     # Multi-Tier Driver Subsystems
│   ├── d1/<type>/<vendor>/<device>/ # Native Rust hardware and software drivers
│   └── d2/<type>/<vendor>/<device>/ # Sandboxed WASM drivers
│
└── apps/                        # Core Applications & Demos
    ├── system_settings/
    └── demo_app/                # Sample .bexapp package with prototxt manifest

```

---

### Layer Responsibilities

* **`idl/` & `tools/fidlc/**`: The single source of truth for interfaces. Bazel builds `fidlc` for the host execution platform (`cfg = "exec"`), parses and validates the BexOS FIDL v1 subset, preserves method-level `@permission` policy expressions, splits protocols into capability groups, and generates deterministic `no_std` Rust bindings. Future backends can plug into the same AST/IR pipeline.

  See `docs/fidl-permissions.md` for the BexOS FIDL v1 reference and `docs/fidl-vs-fuchsia.md` for differences from upstream Fuchsia FIDL.


* **`secure/` and `third_party/trusty/`**: Trusty is the current QEMU secure
  world. Bazel builds TF-A/Trusty firmware with upstream KeyMint, Gatekeeper,
  storage, AVB, AuthMgr FE/BE, and the retained BexOS orchestrator. QEMU owns
  the upstream RPMB proxy lifecycle and persistent development image. The
  removed custom KeyVault/TUI are not shipped; secure ConfirmationUI remains a
  future design until secure display/input ownership exists.


* **`kernel/`**: Compiled for `aarch64-unknown-none`. Contains strictly minimal scheduling, capability routing, MMU configuration, and SMC hooks for heart transplants.


* **`services/appd/`**: Acts as userspace `init`. Manages package manifests, evaluates CEL policies during connection setup, and brokers direct channel connections between client apps and services.

  The userspace and appd flow includes the host-testable broker
  implementation, protobuf manifest schema, build-time prototxt compilation,
  a shared no-std ELF load planner, Bazel-built standalone `appd` ELF and
  boot package artifacts, and the current linked `appd` EL0 bring-up stub. See
  `docs/userspace-and-appd.md`.

  Heart-transplant support adds appd freeze/restore snapshots,
  preserved-RAM metadata, and orchestrator watchdog state. See
  `docs/heart-transplant.md`.

  `teed` is provider-neutral and loads one signed external ABI-v1
  `libbexos_tee_driver.so` selected by its manifest. The real QEMU product
  pins the Trusty driver and fails closed on ABI/probe/transport failure; the
  software driver is selected only by explicit emulated products/tests.


* **`drivers/`**: Split cleanly by execution tier and domain/vendor/device so performance-critical D1 drivers compile natively with zero-copy shared memory, while D2 peripherals compile to `wasm32-unknown-unknown`.

### Current Feature Docs

- [Build and Toolchains](docs/build-and-toolchains.md)
- [GitHub Actions CI](docs/ci.md)
- [Kernel Bring-Up](docs/kernel-bringup.md)
- [Userspace and Appd](docs/userspace-and-appd.md)
- [Heart Transplant](docs/heart-transplant.md)
- [Trusty secure world](docs/rfcs/0051/README.md)
- [QEMU](docs/qemu.md)

### QEMU Developer Instance

Install `qemu-system-aarch64` and `qemu-system-x86_64` on `PATH`, including a
native UI backend (Cocoa on macOS or GTK/SDL on Linux) for workstation launches.
AArch64 remains the default. Firmware source builds support Linux x86_64,
Linux ARM64, and Apple Silicon macOS. Install the
[native prerequisites](third_party/trusty/README.md#native-host-prerequisites),
including GNU make/sed, Python 3, dtc, xxd, and OpenSSL headers.
Refresh the cached firmware before the first launch:

```sh
bazel run //third_party/trusty:refresh_image
bazel run //third_party/trusty:refresh_x86_64_image
```

The **nongui** product provides the existing headless developer instance and is
used by the QEMU E2E suite:

```sh
bazel run --config=aarch64 //device/virtual/qemu/nongui:run
bazel run --config=x86_64 //device/virtual/qemu/nongui:run
```

The **workstation** product opens QEMU's native window with a virtio GPU,
keyboard, and mouse, all attached over PCI:

```sh
bazel run --config=aarch64 //device/virtual/qemu/workstation:run
bazel run --config=x86_64 //device/virtual/qemu/workstation:run
```

Workstation uses the same Trusty boot chain, teed, signed Trusty driver, and
RPMB integration as nongui on AArch64. The x86 product retains its current
boot and Trusty limitations; see [x86_64 support](docs/x86_64-support.md).
Graphics and input support currently consists of configuration and virtual
hardware: no guest graphics/input drivers or compositor are installed, so a
QEMU window does not yet display a BexOS desktop or provide guest input.
Opening the window is separate from reaching debugd readiness.

Each runner owns a temporary writable NVMe disk and its QEMU/RPMB helper
processes. Stop it with Ctrl-C or close the QEMU window. Debug sockets include
both product and architecture:
`/tmp/bexos-qemu-<product>-<architecture>-debugd.sock`. From another terminal,
select the matching product and architecture:

```sh
bazel run --config=aarch64 //device/virtual/qemu/nongui:debugd -- health
bazel run --config=aarch64 //device/virtual/qemu/workstation:debugd -- health
bazel run --config=x86_64 //device/virtual/qemu/workstation:debugd -- health
```

`//device/virtual/qemu/nongui:run_emulated` selects the explicit software-TEE
variant on AArch64 or the Q35 development product with `--config=x86_64`.
The AArch64 run recorded during the original split hit an unresolved symbol in
`teed`; see [validation results](docs/testing-status.md#qemu-product-split-2026-09-06).
The standard `:run` targets do not substitute software
TEE for Trusty. Old `//device/virtual/qemu/nongui:...` labels have been removed.

Standalone x86 Trusty remains separate:

```sh
bazel run //third_party/trusty:run_x86_64
bazel run //third_party/trusty:refresh_x86_64_acceptance_image
bazel test --config=e2e //third_party/trusty:x86_64_acceptance_test
```

Run the complete nongui E2E matrix, including genuine x86 failures, with:

```sh
bazel run //third_party/trusty:refresh_authmgr_acceptance_image
bazel test --config=e2e //testing/e2e/qemu:all_architectures
```

Use `:aarch64` or `:x86_64` for one suite. `--config=e2e` includes
`--keep_going` and optimized builds (`-c opt`). See
[QEMU products](docs/qemu-product.md) for configuration layering and
[testing status](docs/testing-status.md) for verification results.
