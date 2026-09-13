# QEMU product

The graphical workstation bundles configurable [SysUI and UserUI](sysui.md)
packages. Production composition keeps application views hidden until attached
beneath an authenticated desktop.

## Products and shared configuration

```text
device/base/graphics/                  portable graphics package bundle
device/virtual/qemu/base/              shared assembly, images, launchers and keys
  aarch64/                            AArch64 hardware and platform configuration
  x86_64/                             x86_64 hardware and platform configuration
  graphics/                           QEMU graphics bundle and windowed launch policy
device/virtual/qemu/nongui/            headless product; all QEMU E2E artifacts
device/virtual/qemu/workstation/       native QEMU UI with virtio GPU/keyboard/mouse
```

Both products have architecture-selected default targets, `:run`, and `:debugd`.
Their image outputs and product names (`nongui_<arch>` / `workstation_<arch>`)
are separate. Board identity refers to `base/<arch>`. Assembly and image
helpers preserve the existing package contents and security configuration;
workstation adds the portable `graphics` and QEMU `qemu_graphics` bundles.
The graphics bundle contains `splashd` and CPU `scened`; the QEMU graphics bundle
contains the D1 VirtIO-GPU driver. These boot from BootFS and include replacement
archives in graphical update storage.

Launch policies are prototxt compiled by Bazel against `tools/qemu/launch.proto`.
The host runner's `--launch-config` reads the binary policy. Nongui uses
`-nographic`; workstation disables implicit VGA and appends `virtio-gpu-pci`,
`virtio-keyboard-pci`, and `virtio-mouse-pci` after the existing devices. QEMU
selects its native UI backend. Serial/debug transport is retained. Guest CPU rendering and compositor takeover
are implemented; GPU acceleration and general input handling remain future work.

```sh
bazel run --config=aarch64 //device/virtual/qemu/workstation:run
bazel run --config=x86_64 //device/virtual/qemu/workstation:run
bazel run --config=aarch64 //device/virtual/qemu/nongui:run
```

Developer sockets are `/tmp/bexos-qemu-<product>-<architecture>-debugd.sock`.
The matching product's `:debugd` wrapper selects the same socket. The runner
retains guest readiness checks and cleans up owned QEMU/RPMB processes on
normal exit, error, or interrupt. A native window can appear before guest
readiness; it does not establish successful BexOS boot.

## Trusty support

Workstation inherits nongui's current Trusty firmware, authenticated AArch64
boot, teed and signed Trusty driver, trusted applications, security policy,
and RPMB lifecycle. It does not enable secure display/input ownership or
ConfirmationUI. x86 retains the existing integration limitations.

ARM remains the default. `--config=x86_64` selects Q35 and x86 toolchains.
The explicit `//device/virtual/qemu/nongui:virtual_x86_64_development` product
uses software TEE; launch it with `bazel run -c opt --config=x86_64
//device/virtual/qemu/nongui:run_emulated`. COM1 carries early/kernel diagnostics
and virtio-console's `debug0` port carries userspace debug RPC. The harness
combines those channels only for diagnostics, preserving the RPC byte stream.
Integrated x86 Trusty remains unavailable: standard secure launch reports an
explicit development-product diagnostic. See [x86_64 support](x86_64-support.md).

The canonical ARM64 QEMU product uses the pinned upstream Trusty secure world.

- `//third_party/trusty:trusty_qemu_firmware` builds TF-A BL1/BL2/BL31 and
  Trusty/LK with KeyMint, Gatekeeper, secure storage, AVB, AuthMgr FE/BE, and
  the BexOS orchestrator, plus the required hwcrypto, hwbcc, hwcryptohal, and
  system-state providers. Explicitly refresh the gitignored firmware bundle
  with `bazel run //third_party/trusty:refresh_image`.
- `//device/virtual/qemu/nongui:run` stages Trusty as BL32, TF-A-authenticated
  `//third_party/trusty:bl33.bin` as BL33, and signed standard AVB vbmeta beside the
  separately loaded kernel and BootFS. BL33 sets authenticated-boot evidence;
  appd requires the Trusty AVB rollback/lock-state check before pivot. QEMU
  consumers extract the source-built bundle, so secure application changes reach
  the product. Bazel reuses unchanged firmware actions; explicit refresh targets
  remain available for saved recovery snapshots.
  BL33 and its dependencies are always optimized, including when refreshing
  from a default fastbuild configuration. ARM64 bare-metal SHA-256 compression
  also stays optimized for the kernel's BootFS evidence check. The appd service
  artifact likewise optimizes its loader and dependencies. The focused
  `bazel test //boot/bl33:verification_boot_test --test_tag_filters=requires-qemu`
  regression verifies the workstation partitions within 15 seconds.
  `//device/virtual/qemu/nongui:run_emulated` remains the explicit software
  TEE launcher, with architecture-specific development configuration. See
  [validation results](testing-status.md#qemu-product-split-2026-09-06) for the
  original split’s software-TEE startup limitation.
- The Trusty QEMU harness launches the upstream `rpmb_dev` as an owned child,
  waits for its socket, attaches it as the `rpmb0` virtio-serial port, and
  terminates it whenever QEMU terminates. Its `RPMB_DATA` image is preserved
  across boots of an instance; tests may nominate an explicit persistent image
  with `BEXOS_QEMU_RPMB_STATE`.
- QEMU keys and RPMB data are development-only. Hardware ports must provide an
  authenticated BL33 load, provisioned AVB root, eMMC/UFS RPMB, and working
  Trusty SMC/shared-memory transport; absence fails closed.

`teed` is provider-neutral and the product manifest selects the signed Trusty
ABI-v1 driver. Only privileged system clients can open the typed KeyMint,
Gatekeeper, storage, AVB, AuthMgr, and orchestrator ports. Ordinary apps do not
receive raw Trusty access. Interrupted calls return explicit transport errors;
session/catalog state is preserved during heart transplant. Operations are
quiesced before handover; interrupted operations return errors instead of
automatically retrying potentially committed writes.

Trusty secure storage owns KeyMint blobs, Gatekeeper handles and throttling
state, AVB rollback/lock state, and AuthMgr state. Gatekeeper hardware-auth
tokens authorize KeyMint directly through their shared secure secret. AuthMgr
FE/BE provides upstream DICE and secure-service connection authorization; it is
not a Gatekeeper token broker. The orchestrator remains dedicated to Trusty
core/update lifecycle and heart-transplant coordination.

ConfirmationUI is intentionally not enabled. A future secure confirmation UI
requires exclusive secure display and input ownership that this QEMU product
does not implement.

The QEMU E2E suite is tagged `requires-qemu`; run it with:

```sh
bazel test -c opt //testing/e2e/qemu/... --test_tag_filters=requires-qemu --test_output=errors
```

See `third_party/trusty/README.md` for bundle contents and the separate AuthMgr
acceptance refresh command, and `testing-status.md` for actual verification.

RFC 0064 package-resolution acceptance has not passed on either architecture.
ARM boots source-built Trusty and provisions sealed package credentials, but
verified artifact delivery, consumer integration, replacement and reboot
persistence remain unpassed. Final-tree nongraphical and workstation assemblies
on both architectures also remain to be checked. See the
[package-resolution gaps](rfcs/0064/CURRENT.md#current-gaps); the presence of a
product target or fixture is not acceptance evidence.

## Graphical boot UI

The workstation graphics bundles package the D1 VirtIO-GPU driver, `splashd`,
and a minimal CPU `scened`, with replacement archives and generated FIDL.
See [boot UI](bootui.md) for ownership, boot ordering, migration, validation
commands, and the boundary with the full desktop design.
