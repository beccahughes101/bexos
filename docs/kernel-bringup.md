# Kernel Bring-Up

BexOS builds one bare-metal AArch64 kernel image at `//kernel`.

## Included

- An AArch64 `_start` path with per-CPU boot stacks. CPU0 clears `.bss` and
  enters Rust `kernel_main`; secondary CPUs wait for the boot barrier, install
  exception vectors, enter a Rust secondary hook, publish boot progress, and
  join the scheduler idle path after CPU0 releases global init.
- Early PL011 UART output at QEMU `virt` UART0, `0x0900_0000`.
- A page-frame allocator seeded from the validated boot handoff, with BootFS,
  update/snapshot, Trusty secure DRAM, and kernel/heap ranges reserved.
- Initial EL1 translation setup using the ARMv8 4 KiB translation regime,
  including EL0-accessible pages for the linked `appd` entry and stack.
- GICv2-compatible interrupt setup and ARM generic physical timer ticks.
- An allocation-backed SMP-aware scheduler with task states, fixed quanta,
  CPU affinity masks, per-CPU current tasks, priority-aware round robin,
  blocked wakeups, and resource-group CPU accounting.
- A config-driven QEMU SMP topology from `device.prototxt`; the current board
  sets `max_cpus: 4` and proves CPU0-CPU3 reach their boot paths.
- Allocation-backed point-to-point IPC channels with optional capability metadata.
- Generated kernel FIDL control services behind the EL0 `svc #1` dispatcher.
- Process, VM-space, VMO, mapping, thread, futex, interrupt, and checkpoint
  records in the shared kernel control plane.
- The integrated MMU-on userspace handoff and heart-transplant prototype.
  Secure confirmation UI is intentionally not present until a board can grant
  Trusty exclusive display and input ownership.

## Memory Map

The linker script places the kernel at `0x4020_0000`, above the low QEMU `virt`
RAM area used for firmware-generated data such as the device tree. QEMU loads
the BootFS image and handoff descriptor separately.

```text
0x0800_0000  GIC distributor
0x0801_0000  GIC CPU interface
0x0900_0000  PL011 UART0
0x4010_0000  QEMU fallback BootHandoff descriptor
0x4020_0000  kernel image start
0x4200_0000  heart-transplant replacement kernel slot
0x4300_0000  heart-transplant snapshot records
0x4400_0000  external BootFS image
0x4800_0000  free-memory limit
0x6000_0000  QEMU RAM end
```

The initial MMU configuration maps:

- `0x0000_0000..0x4000_0000` as device memory for MMIO.
- `0x4000_0000..0x8000_0000` as normal write-back RAM for EL1.
- The linked `appd` entry page as EL0 readable/executable.
- The linked `appd` stack pages as EL0 readable/writable and execute-never.

## Boot Sequence

1. `_start` reads `MPIDR_EL1` and assigns a dedicated boot stack for each CPU
   in the configured topology.
2. The boot core clears `.bss`, releases the secondary boot barrier, and enters
   `kernel_main`; secondary CPUs enter `secondary_kernel_main`.
3. Exception vectors are installed in `VBAR_EL1`.
4. `kernel_main` accepts a valid versioned handoff pointer from `x0`, or falls
   back to the QEMU descriptor at `0x4010_0000`, then waits for the configured
   CPU boot mask and logs boot-path readiness for each CPU.
5. `kernel_main` initializes UART, allocator, the initial MMU tables, IPC, GIC,
   and CPU0's timer, then releases secondary CPUs to initialize their local
   GIC CPU interface and timer.
6. The kernel runs the heart-transplant prototype.
7. The kernel parses the external BootFS image, loads
   `/boot/pkg/bexos.platform.appd/bin/appd`, creates the initial `appd` process
   and VM space, sends the BootFS VMO over the bootstrap channel, starts its
   first thread in the scheduler, and records scheduler tick decisions. The
   bare-metal EL0 runtime still runs on CPU0 while secondary CPUs idle with
   interrupts enabled.
8. The kernel launches BootFS-provided `appd` at EL0 with the MMU still enabled.
   `svc #0` remains the appd readiness syscall, and `svc #1` dispatches
   generated kernel FIDL calls.

UART output uses subsystem markers such as `kernel:`, `heart-transplant:`,

## Validation

```sh
BAZELISK_HOME=/private/tmp/bexos-bazelisk-cache bazel build --config=aarch64-none //kernel
BAZELISK_HOME=/private/tmp/bexos-bazelisk-cache bazel run //tools/qemu:kernel_boot
```

The QEMU smoke test is `//testing/e2e/qemu/kernel:kernel_smoke_test`. It runs
QEMU with the `MAX_CPUS` value emitted from the device prototxt and requires
`kernel: cpu0 boot path ready` through the configured last CPU marker.
