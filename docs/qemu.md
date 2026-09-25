# QEMU

Two products share the QEMU base: `//device/virtual/qemu/nongui:run` is the
headless developer/E2E product, and `//device/virtual/qemu/workstation:run`
opens the native QEMU UI with virtio GPU, keyboard and mouse devices. Both
accept `--config=aarch64` and `--config=x86_64`; AArch64 is the default.
Workstation preserves the current Trusty setup and adds no guest graphics or
input drivers yet. See [QEMU products](qemu-product.md) for the layout,
launch policy, and current limitations. Old root QEMU targets are removed.

The nongui launcher is `//device/virtual/qemu/nongui:run`, with ARM by default and `--config=x86_64` for Q35. `:debugd` selects the matching socket. The ARM behavior below remains applicable to ARM; x86 uses the Bazel-built Multiboot2 adapter. See [x86_64 support](x86_64-support.md) for both launch commands, standalone Trusty, firmware refresh, and validation status.

Build and run the bounded AArch64 storage bootstrap with Bazel:

```sh
bazel run //device/virtual/qemu/nongui:run
bazel test --config=e2e //testing/e2e/qemu/kernel:kernel_smoke_test
bazel test --config=e2e //testing/e2e/qemu/bexfs:bexfs_sys_state_e2e_test
bazel test --config=e2e //testing/e2e/qemu/bexfs:debugd_app_registry_e2e_test
bazel test --config=e2e //testing/e2e/qemu/update:all_architectures
```

The wrapper requires `qemu-system-aarch64`, creates a temporary writable copy
of the generated GPT image, and always terminates and reaps QEMU. It starts
QEMU `virt` with the NVMe controller and uses QEMU's generic loader to place
the external BootFS and versioned handoff descriptor in RAM. It does not use an
initrd or a kernel-embedded BootFS.

The QEMU board currently runs with 1 GiB of guest RAM. The upper half contains
TF-A's authenticated BL33 load address at `0x6000_0000`; BexOS still reserves
its kernel, BootFS, policy, evidence, and update ranges explicitly before the
normal allocator is enabled. This leaves enough
room for the 64 MiB early BootFS reservation plus appd, early drivers,
`debugd`, `teed`, `updated`, post-storage system-image driver binding,
the storage-preinstalled networkd/netstackd services, vswitchd, service replacement archives, and
the disk-only verifier heaps.

The base BootFS is assembled once and then passed through a reusable verified
package-tree overlay. Product-authorized SDK services and drivers therefore
enter `/boot/pkg/<package-id>/` without duplicating the base entry list; appd
starts them from their signed process wave and evaluates the signer/role
provenance recorded by product assembly.

CPU topology is declared in `device/virtual/qemu/base/aarch64/device.prototxt`. Bazel passes
`cpu_topology.max_cpus` through the boot handoff generator, emits it as
`MAX_CPUS` in `boot_layout.sh`, and the QEMU harness uses that value for
`-smp`. The current board sets `max_cpus: 4`.

The full storage E2E test boots the same temporary disk twice. The guest writes and
syncs `SYS_STATE` and `/data` during the first boot; the second boot verifies
both records. After QEMU exits, the host inspector opens the image read-only,
checks those records, and confirms the disk-only verifier is still readable
from `/pkg`. Missing QEMU, timeout, missing required serial evidence, or a
failed inspection fails the test.

The debugd E2E target boots once with QEMU serial attached to a temporary UNIX
socket instead of stdio. The harness waits for
`debugd: QEMU socket transport ready`, sends BXD1-framed protobuf requests,
and verifies `HealthCheck`, `ListProcesses`, and `ExecCommand(debugd.version)`
responses before allowing the normal guest persistence checks to finish.

The app-registry debugd E2E boots the same debug transport and checks that
`debugd` can list appd registry records. It verifies the signed
`bexos.platform.storage_verify` package is marked running and protected, then
confirms protected uninstall requests are denied by appd policy.

The update-engine E2E boots the debug transport, streams signed BexOS update
manifests and artifacts through debugd, installs a verified app update through
appd, exercises `bexctl tee` info/app/session/invoke management through
debugd and `teed`, rejects a bad signed TEE image before it reaches `teed`,
routes a verified `TEE_IMAGE` update to `bexos.tee.TeeManager.UpdateTeeCore`,
then applies a verified microkernel platform update through
`KernelDebugControl.ApplyPlatformUpdate`. The microkernel artifact is
`//kernel:update_kernel`, separately linked at `0x42000000`. The test requires
old/new kernel markers, preserved process/app identities, post-takeover
health/status replies, and an allocation from reclaimed old-kernel memory.
Secondary CPUs enter the normal idle path after boot, but only CPU0 changes
kernels during the current replacement flow.

This path preserves live EL0 memory and IPC state; it does not reboot or relaunch
apps. The fixed replacement slot supports one takeover per boot. Secure World
TEE loading and core-update routing go through provider-neutral D1 `teed` and
its external ABI-v1 driver library. The Trusty product fails closed on
ABI/probe/transport failure; software emulation is selected only by explicit
emulated products/tests. QEMU owns the upstream RPMB proxy and attaches its
persistent development image as `rpmb0`. Physical-board RPMB and Trusty
board enablement, SMP ownership transfer during kernel replacement, and DMA migration
remain future work. See
[Heart Transplant](heart-transplant.md) for the ABI and memory layout.


The lazy-keychain E2E scenario lives under `//testing/e2e/qemu/elf`. It installs
the ELF probe bundle, verifies keychaind is absent before demand, connects to the
Keychain service, verifies a single shared keychaind instance for multiple
clients, drops the clients, waits for idle exit, reconnects, and verifies the
persisted secret is available after relaunch. The integrated AArch64 target is
`//testing/e2e/qemu/elf:lazy_keychain_test_aarch64`; the current x86_64 coverage
uses the development boot product at
`//testing/e2e/qemu/elf:lazy_keychain_development_test_x86_64` because the
standard x86_64 secure target depends on the local authenticated EFI firmware
cache.

Expected guest evidence includes:

- `userspace: entering el0 appd`
- PCI, PL011, NVMe, and BexFS EL0 readiness
- `/pkg mounted read-only`
- `pivot complete; /boot removed; BootFS reclaimed pages=`
- `storage-verify: disk-only ELF ran`
- `debugd: QEMU socket transport ready`
- `update-engine: service ready; debugd command bridge active`
- `teed: service ready`
- `debugd: tee proxy connected`
- `netstackd: service ready`
- `vswitchd: service ready`
- `networkd: service ready`
- `appd: launched preinstalled service package=bexos.service.netstackd`
- `appd: registry launched signed app package=bexos.platform.storage_verify`
- `appd: app lifecycle registry ready for debugd`
- `appd: guest persistence and disk-only application verified`

`//device/virtual/qemu/nongui:qemu_nvme_gpt.img` is a Bazel output. It preserves the
partition order in [the storage-bootstrap design](rfcs/0009/README.md), with
32 MiB of encrypted BexFS for `SYS_STATE` and 512 MiB of encrypted BexFS for
`STORAGE`. BexFS divides the volume between two namespace snapshots, so each
snapshot has just under 256 MiB for packages, data, and metadata. The QEMU
`STORAGE` volume contains `pkg/` and `data/`; `vfsd`
owns the managed package-store mount for disk app launch and debug installs
while `appd` still mounts `/data` writable for the current QEMU verifier path. The image
preinstalls storage verifier, crypto, VirtIO-Net, vswitchd, netstackd,
networkd, timed, and jobd
package archives and uses an explicitly QEMU-only test key. Keychaind is present
as an installable package archive and exposed through its lazy manifest; it is not
started by the storage service wave. Production A/B boot selection, signed
BootFS bundle conversion, interrupt-driven I/O, and production key handling are
outside this target.

To inspect a generated image, use the Bazel target rather than host mutation:

```sh
bazel run --cxxopt=-std=c++17 --host_cxxopt=-std=c++17 //tools/image:bexfs_image -- \
  --inspect --image /absolute/path/to/qemu_nvme_gpt.img \
  --partition SYS_STATE --label SYS_STATE \
  --key-file /absolute/path/to/qemu_bexfs_test.key \
  --path boot_state.bin --sys-state
```
