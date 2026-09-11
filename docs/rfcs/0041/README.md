# RFC 0041: Repository layout and Bazel product assembly

- Created: 2026-08-31T15:34:10-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

Shared board configuration, product definitions, and tiered driver directories organize the repository. Starlark composes existing protobuf schemas, while Bazel platforms and transitions select execution environments.

## Current implementation status

The implemented layout includes shared QEMU configuration under
`device/virtual/qemu/base/{aarch64,x86_64}`, portable graphics bundles under
`device/base/graphics`, QEMU graphics configuration under
`device/virtual/qemu/base/graphics`, and concrete `nongui` and `workstation`
products. Product Starlark compiles through Bazel into the existing protobuf
assembly pipeline. Manifests and launch policies remain prototxt.

Workstation now configures QEMU's native display, virtio GPU, keyboard, and
mouse, preserving the current Trusty support. Guest compositor, graphics/input
drivers, system applications, and smartphone products remain future design.
See [current QEMU products](../../qemu-product.md) for the implemented
configuration; the designs below describe the longer-term system.

Prototxt remains the configuration format. Starlark adds a higher-level composition language through [starlark-rust](https://github.com/facebook/starlark-rust) and Bazel rules. It compiles to protobuf using the same schema as prototxt.

Current repo layout:

bexos/
├── MODULE.bazel                 \# Bzlmod dependencies (rules\_rust, rules\_cc, etc.)
├── .bazelrc                     \# Multi-target platform flags & rustc settings
├── build/
│   ├── platforms/               \# Platform definitions (aarch64\_none, wasm32)
│   ├── toolchains/              \# Cross-compilers (LLVM, Rust, WASM)
│   └── rules/                   \# Starlark macros (.fidl codegen, .bexapp packaging)
│
├── docs/                         \# Always document
├── idl/                         \# Central FIDL Interface Definitions
│   ├── bexos/
│   │   ├── hardware/            \# bexos.hardware.Location, Camera, etc.
│   │   ├── system/              \# bexos.system.AppService, Orchestrator
│   │   └── media/               \# bexos.media.AudioSink, WebGPU
│   └── BUILD.bazel
│
├── tools/                       \# Host-Side Build & Code Generation Tools
│   ├── fidlc/                   \# Forked FIDL compiler (+@permission & CEL AST split)
│   ├── qemu/                    \# Bazel wrappers for QEMU bring-up
│   ├── packager/                \# .bexapp bundler & manifest compiler (prototxt \-\> bin)
│   └── cert-tools/              \# VMC/BIMI brand verifier & signing utilities
│
├── secure/                      \# BexOS Trusty integration and orchestrator TA
│   ├── orchestrator/            \# Root-of-Trust supervisor, A/B fallback, watchdog
│   ├── confirmation/            \# FUTURE: secure UI after display/input ownership
│   └── rpmb\_store/              \# Last-Known-Good manifest state storage
│
├── kernel/                      \# Non-Secure EL1 Microkernel (no\_std Rust)
│   ├── core/                    \# Testable no\_std allocator, MMU, scheduler, IPC logic
│   ├── src/
│   │   ├── arch/aarch64/        \# MMU 4-level paging, GIC, context switching
│   │   ├── memory/              \# Physical frame allocator, grant tables
│   │   ├── ipc/                 \# Channel endpoints & zero-copy buffer handoff
│   │   ├── sched/               \# Preemptive scheduler & resource group (cgroup) hooks
│   │   └── criu/                \# Heart-transplant freeze/restore dispatch
│   └── BUILD.bazel
│
├── services/                    \# Userspace System Servers (Native Rust)
│   ├── app\_service/             \# init, manifest manager, CEL evaluator, broker
│   ├── vmm/                     \# MicroVM monitor (Linux/Android guest isolation)
│   ├── compositor/              \# UI surface manager & WebGPU pipeline
│   └── vfs/                     \# Capability-based encrypted storage broker
│
├── drivers/                     \# Multi-Tier Driver Subsystems
│   ├── d0\_kernel/               \# Platform timer, early panic/debug sinks
│   ├── d1\_native/               \# Native Rust high-throughput drivers (PCI, UART, NVMe)
│   │   └── linux\_shim/          \# C compatibility layer for wrapping Linux drivers
│   └── d2\_wasm/                 \# Sandboxed WASM modules (HID, Serial, Sensors)
│
└── apps/                        \# Core Applications & Demos
    ├── system\_settings/
    └── demo\_app/                \# Sample .bexapp package with prototxt manifest

Repo layout changes:

- device will now be the following format:
  - /device/base
    - :base (core config for any non GUI device)
    - :graphical (core config for any GUI device)
  - /device/\<manufacturer\>/\<board\>/product
    - For example, QEMU would use the following:
    - /device/virtual/qemu with the following products using starlark to create configuration libraries shared across products
      - base\_aarch64 \- product for non-GUI QEMU ARM devices
      - base\_graphics\_aarch64 \- product for graphics QEMU ARM devices
      - virtual\_aarch64 \- device for QEMU ARM virtual device non-graphical
      - workstation\_aarch64 \- device for QEMU ARM virtual device workstation build
      - smartphone\_aarch64 \- device for QEMU ARM virtual device smartphone
- drivers/\<level\>/\<type\>/\<manufacturer\>/\<device\>
  - level: D1/D2
  - type: audio/nic

Bazel platforms (`constraint_setting` and `platform` definitions) select cross-compilation targets across execution tiers.

In a heterogeneous OS like BexOS, a single build graph mixes completely different environments:

* **Microkernel (EL1):** `aarch64` / `x86_64`, `no_std`, bare-metal (`target_os = "none"`).
* **Secure World (Trusty):** `aarch64`, TrustZone secure monitor and upstream
  KeyMint/Gatekeeper/storage/AVB/AuthMgr applications.
* **D1 Drivers / Native Daemons:** `aarch64` / `x86_64`, `std` or `no_std`, BexOS user ABI.
* **D2 Drivers / WASM Apps:** `wasm32-wasip2`, sandboxed linear memory.
* **Build Tools / Host Tools (`fidlc`, `packager`):** Host Linux/macOS.

Bazel platforms and transitions build the entire system from one top-level command without manual toolchain switching or shell scripts.

## Platform & Constraint Hierarchy (`build/platforms/`)

Define explicit constraints for OS tiers and environments alongside standard CPU architectures:

```python
# //build/platforms/constraints/BUILD.bazel
package(default_visibility = ["//visibility:public"])

constraint_setting(name = "execution_tier")

constraint_value(
    name = "tier_d0_kernel",
    constraint_setting = ":execution_tier",
)

constraint_value(
    name = "tier_d1_native",
    constraint_setting = ":execution_tier",
)

constraint_value(
    name = "tier_d2_wasm",
    constraint_setting = ":execution_tier",
)

constraint_value(
    name = "tier_secure_world",
    constraint_setting = ":execution_tier",
)

```

Now combine CPU constraints with system execution tiers:

```python
# //build/platforms/BUILD.bazel
package(default_visibility = ["//visibility:public"])

# D0 Kernel: Bare-Metal ARM64 (no_std)
platform(
    name = "kernel_aarch64",
    constraint_values = [
        "@platforms//cpu:aarch64",
        "@platforms//os:none",
        "//build/platforms/constraints:tier_d0_kernel",
    ],
)

# D1 / Services: Native Userspace ARM64
platform(
    name = "userspace_aarch64",
    constraint_values = [
        "@platforms//cpu:aarch64",
        "//build/platforms/constraints:bexos",
        "//build/platforms/constraints:tier_d1_native",
    ],
)

# D2 / WASM: Sandboxed Guest Modules
platform(
    name = "wasm32_guest",
    constraint_values = [
        "@platforms//cpu:wasm32",
        "@platforms//os:wasi",
        "//build/platforms/constraints:tier_d2_wasm",
    ],
)

# Secure Monitor: Trusty / TrustZone
platform(
    name = "secure_aarch64",
    constraint_values = [
        "@platforms//cpu:aarch64",
        "//build/platforms/constraints:tier_secure_world",
    ],
)

```

## Multi-Platform Image Assembly via Bazel Transitions

The key advantage of Bazel platforms is **configuration transitions (`cfg`)**. A top-level product rule can depend on components compiled for completely different architectures and tiers in a single invocation:

```python
# //build/rules/product_image.bzl

# Transition to build the microkernel for bare-metal
def _kernel_transition_impl(settings, attr):
    return {"//command_line_option:platforms": "//build/platforms:kernel_aarch64"}

# Transition to build WASM drivers/apps for wasm32-wasi
def _wasm_transition_impl(settings, attr):
    return {"//command_line_option:platforms": "//build/platforms:wasm32_guest"}

# Transition for userspace servers (app_service, vfsd, etc.)
def _userspace_transition_impl(settings, attr):
    return {"//command_line_option:platforms": "//build/platforms:userspace_aarch64"}

```

## Product definitions using platforms

Product rules in `/device/virtual/qemu/` select the correct Rust toolchain for each dependency through Bazel:

```python
# //device/virtual/qemu/BUILD.bazel
load("//build/rules:system_image.bzl", "bexos_image")

bexos_image(
    name = "smartphone_aarch64_image",
    kernel = "//kernel:core",                      # Automatically built with :kernel_aarch64
    secure_monitor = "//secure/orchestrator",      # Automatically built with :secure_aarch64
    system_servers = [
        "//services/app_service",                  # Automatically built with :userspace_aarch64
        "//services/compositor",
        "//services/vfs",
    ],
    d1_drivers = [
        "//drivers/d1/gpu/virtio:gpu",            # Automatically built with :userspace_aarch64
        "//drivers/d1/nic/intel:e1000",
    ],
    d2_drivers = [
        "//drivers/d2/hid/generic:touchscreen",   # Automatically built with :wasm32_guest
        "//drivers/d2/sensors/generic:accel",
    ],
    config = ":smartphone_aarch64_config",         # Compiled from Starlark -> Protobuf
)

```

## Build properties

* **Single-Command Builds:** A developer or CI runs:
```bash
bazel build //device/virtual/qemu/nongui:smartphone_aarch64_image

```


Bazel resolves the entire graph, compiling TrustZone EL3, microkernel EL1, userspace EL0 daemons, and WASM packages using their respective cross-compilers in parallel.
* **Hermetic Caching:** Changing a line in a D2 WASM driver only invalidates the WASM component and image packaging step; the D0 kernel and native drivers stay cached.
* **Seamless Board Porting:** Adding `//device/rockchip/rk3588` or an `x86_64` workstation requires only swapping the platform constraints and product driver list—the dependency graph automatically rebuilds with the correct targets.

Starlark composition through `starlark-rust` builds on the existing protobuf/prototxt schemas and reorganized `/device` and `/drivers` layout. Related patterns include Fuchsia’s assembly/product-bundle definitions and Android’s Soong/Starlark migration. The design retains one schema source of truth.

## Strengths of the New Architecture

* **Schema Continuity:** Compiling Starlark configurations down to binary Protobuf (`.binpb` / `.pb`) preserves existing runtime parsers. Services like `app_service` or `packager` do not need Starlark interpreters embedded into production/device builds.
* **Composition Without Schema Drift:** Pure `prototxt` quickly suffers from copy-paste boilerplate across variants (e.g., QEMU base vs. graphics vs. smartphone). Starlark provides functions, loops, and inheritance to compose product definitions cleanly.
* **Deterministic Hermetic Builds:** Because `starlark-rust` is an isolated, hermetic, side-effect-free dialect of Python, the build graph remains 100% reproducible and cacheable by Bazel.
* **Clean Tree Ergonomics:** The `/device` structure establishes clear hierarchical sharing (board definitions vs. product configurations), and `/drivers/<level>/<type>/<manufacturer>/<device>` makes capability domains (D1 native vs. D2 WASM sandboxed) instantly visible.

## Configuration refinements

**1. Keep Starlark Host-Side (Two-Phase Assembly)**
Ensure `starlark-rust` runs strictly as a host-side tool action inside Bazel rules (`tools/config_compiler`).

* Developers write `.star` configurations $\longrightarrow$ Host rule executes `starlark-rust` $\longrightarrow$ Emits standard `.binpb` / `.pb` $\longrightarrow$ Bundled into `.bexapp` / boot images.
* This avoids pulling dynamic scripting runtimes into `services/` or `kernel/`.

**2. Standardize Driver Tier Naming in the Tree**
The implemented constraint values are `tier_d0_kernel`, `tier_d1_native`,
`tier_d2_wasm`, and `tier_secure_world`; the filesystem hierarchy is shortened
to `d1` and `d2`. The long-term layout note below uses:
`drivers/<level>/<type>/<manufacturer>/<device>` (e.g., `level: D1/D2`).

* Enforce lowercase consistent directory conventions:
```
drivers/
├── d1/
│   ├── nic/intel/e1000/
│   └── audio/virtio/sound/
└── d2/
    └── hid/generic/mouse/

```


* Keeping the level prefix (`d1`, `d2`) consistent makes Bazel visibility restrictions (`package_group`) trivial to configure (e.g., restricting raw physical MMIO capabilities only to `//drivers/d1/...`).

**3. Product Inheritance Model in Starlark**
Organize Starlark product definitions into shared library files (`.bzl` / `.star`) with standard extension points:

```python
# //device/virtual/qemu/products.star
load("//device/base:configs.star", "base_system", "graphical_system")

def qemu_smartphone_aarch64():
    return graphical_system(
        arch = "aarch64",
        board = "//device/virtual/qemu/nongui:board",
        drivers = [
            "//drivers/d1/gpu/virtio:gpu",
            "//drivers/d2/hid/generic:touchscreen",
        ],
        system_apps = [
            "//apps/system_settings",
            "//apps/sysui",
        ],
        screen_profile = { "width": 1080, "height": 2400, "dpi": 420 },
    )

```

**4. Add Schema Validation (`protoc` / CEL assertions)**
Take advantage of Starlark's execution phase to run compile-time assertions on product configurations before producing binary protos (e.g., verifying that if `graphical = True`, a valid `compositor` and display driver are present in the package manifest list).

## Extending the board and product matrix

The repository layout and configuration pipeline use Bazel’s execution model to support additional boards (QEMU and physical hardware) and product forms (base headless, workstation, and smartphone).
