# RFC 0010: Board and platform configuration

- Created: 2026-08-26T20:52:50-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

Board-owned prototxt manifests define hardware, boot contents, platform policy, and software access tiers. Bazel compiles configuration, while the kernel enforces delegated hardware access.

## Design overview

BexOS hardware targets live under `//device/<target>`. The current bringup
headless product is `//device/virtual/qemu/nongui`, matching the AArch64 kernel and Bazel
toolchain configuration.

Each board directory owns four source manifests:

- `device.prototxt`: board name, architecture, boot command line, memory layout,
  and secure-world mode.
- `bootfs_manifest.prototxt`: packages that must be present before persistent
  storage is available, grouped by startup wave. For QEMU AArch64, package
  labels in this file are checked against declared Bazel inputs before
  `bootfs.img` is assembled, so missing driver targets fail the board build.
- `system_image.prototxt`: packages provisioned into the base system image.
- `platform.pcfg.prototxt`: signed platform policy for runners, TEE, and driver
  hardware access.

Boards may also own product assembly sources:

- `qemu_hardware.aib.prototxt`: QEMU hardware package membership and desired
  `BOOTFS`/`SYSTEM_IMAGE` placement; portable packages live in
  `//device/base:base`.
- `products.star`: selected bundles, board label, platform config label, and
  per-component config overrides, compiled host-side to protobuf.

Driver package manifests may also include `driver_info` and `bind_rules`.
Board images compile those prototxt manifests into `.bexmanifest` entries; at
boot, appd indexes the bind rules and only starts matching hardware child
drivers after bus discovery registers a compatible device node.

Bazel is the source of truth for validating and compiling these manifests:

```bash
bazel build --cxxopt=-std=c++17 --host_cxxopt=-std=c++17 //device/virtual/qemu/nongui:all
```

The explicit C++17 flags override a user-level macOS Bazel rc that sets C++14;
protobuf/abseil require C++17.

## Schema

Board and policy protos live in `//idl/bexos/platform`:

- `assembly.proto` defines `AssemblyInputBundle`, `ProductDefinition`,
  `ComponentConfigOverride`, and the emitted assembly package index.
- `config.proto` defines `PlatformConfig`, `RunnerPolicy`, `TeePolicy`, and
  `DriverPolicy`.
- `device.proto` defines `Device`, `BootfsManifest`, and `SystemImageManifest`.

`//build/rules:assembly.bzl` wraps `protoc` and `//tools/assembly:bexos_assembly`
so product assembly remains hermetic and source-controlled as prototxt.

The QEMU AArch64 board config uses:

```protobuf
device_name: "qemu_aarch64"
architecture: ARCH_AARCH64

hardware_features {
  has_pci: true
  secure_world: EMULATED
}

memory_layout {
  kernel_base_phys: 0x40080000
  bootfs_reserved_mb: 64
}

boot_configuration {
  default_cmdline: "console=ttyAMA0 log_level=debug early_watchdog=off"
}
```

## Policy Flow

`platform.pcfg` is decoded by `appd` at boot. The same policy gates
startup waves, manual launches, driver binding, and the kernel process creation
metadata passed over `SystemPrivileged.CreateProcess`.

Runner policy:

- WASM and web runners are allowed for standard app profiles.
- ELF is allowed for `PlatformCore` and `SystemHardware` packages.
- Consumer ELF requires `allow_native_elf_runner` and a matching package/signer
  allowlist entry.
- Android and Nix requests route to the MicroVM policy when enabled; the runner
  remains unsupported until the MicroVM launcher exists.

TEE policy:

- `enforce_secure_boot` requires the boot chain to measure the platform config.
- `rpmb_anti_rollback` requires monotonic config version checks.
- `orchestrator_verification` pins the secure-world orchestrator measurement.
- `allowed_trusted_apps` lists TAs that normal-world services may request.

Driver policy:

- Tier 1 allowlist entries grant direct MMIO, contiguous physical DMA, and IRQ
  binding to matching package/signer pairs.
- Tier 2 drivers run with isolated hardware access when strict IOMMU rules are
  enabled and raw MMIO is disabled.
- Unmatched drivers are rejected and isolated.
- Driver bind rules only select candidate packages; `platform.pcfg` still makes
  the final hardware-access decision before appd launches a driver.

## Kernel Enforcement

`SystemPrivileged.CreateProcess` carries:

- process name
- resource group
- package ID
- hardware access tier: `NONE`, `ISOLATED`, or `DIRECT`

The kernel stores that tier in `ProcessRecord`. Direct hardware syscalls are then
gated inside kernel services:

- `VirtualMemory.CreateVmo` rejects `CONTIGUOUS_PHYS` and `CACHE_POLICY_UC`
  unless the current process has `DIRECT` hardware access.
- `SystemPrivileged.BindInterrupt` rejects non-direct callers.

This makes appd the policy decision point and the kernel the final
enforcement point for MMIO, DMA, and IRQ capabilities.

## Update Policy

Heart Transplant updates must treat `platform.pcfg` like kernel and driver
images:

- incoming configs must have a monotonic version greater than or equal to the
  RPMB-sealed version;
- updates may add restrictions, but must not silently downgrade runner, TEE, or
  driver policy;
- rollback restores the last-known-good config hash alongside the component
  manifest.
