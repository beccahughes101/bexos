# RFC 0049: Boot handoff ABI

- Created: 2026-09-01T14:01:49-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

BootHandoff transfers memory topology, BootFS location, update reservations, verified-boot evidence, and CPU topology from the loader to the kernel through a versioned ABI.

## Design overview

`BootHandoff` is the ABI contract and data bridge passed from an early
bootloader, firmware path, or QEMU loader to the BexOS microkernel. It transfers
the RAM topology, externally loaded BootFS image, update/snapshot reservation,
verified-boot evidence, and CPU topology before the kernel starts the normal
allocator-backed runtime.

## Current ABI

The current ABI is version 3 and is defined in `lib/boot` as a sixteen-word
little-endian `repr(C)` record:

```rust
BootHandoff {
    magic: HANDOFF_MAGIC,             // "BEXRAM01"
    version: BOOT_HANDOFF_VERSION,    // 3
    ram_start: RAM_START,             // 0x4000_0000
    ram_end: RAM_END,                 // 0x8000_0000
    bootfs_addr: BOOTFS_ADDR,         // 0x4400_0000
    bootfs_len,                       // actual loaded BootFS image length
    update_base: UPDATE_BASE,         // 0x4200_0000
    update_len,                       // through UPDATE_SNAPSHOT_END
    max_cpus,
    entropy_seed_valid,               // 0 or 1
    entropy_seed: [u64; 4],           // optional 256-bit boot entropy
    boot_evidence_addr,               // 0x43f0_0000
    boot_evidence_len,
}
```

QEMU loads the descriptor at fixed `HANDOFF_ADDR` (`0x4010_0000`) with a
generic loader device. The TF-A-authenticated BL33 verifier authenticates AVB
vbmeta and its kernel, BootFS, and platform-policy descriptors, commits Trusty
AVB rollback/lock state, and overwrites the evidence region with version-2
results before entering the kernel. Firmware and future boot shims may instead pass a pointer
to a valid descriptor in register `x0`. The kernel supports both modes: a valid
aligned RAM pointer in `x0` wins, otherwise the kernel validates the descriptor
at `HANDOFF_ADDR`.

The descriptor is rejected if:

- magic or version is unsupported;
- effective CPU count is zero or greater than `MAX_BOOT_CPUS`;
- RAM, BootFS, or update ranges are unaligned, out of RAM, overflow, or overlap
  the kernel image;
- the update reservation overlaps BootFS.
- version 3 marks entropy present but supplies an all-zero seed.

Version 1 descriptors are accepted only for compatibility and imply one CPU.
Version 2 descriptors remain accepted and imply no boot entropy seed.

## Boot Handoff Pipeline

```text
1. Loader / firmware
   - discovers or declares RAM topology;
   - authenticates and enters BL33 at `0x6000_0000`;
   - loads the kernel image at KERNEL_START for BL33 to verify;
   - loads BootFS at BOOTFS_ADDR;
   - writes the v3 BootHandoff descriptor, including a boot entropy seed when
     available;
   - loads signed standard vbmeta at `VBMETA_ADDR`;
   - relies on fixed `HANDOFF_ADDR` for the current QEMU verifier.

2. AArch64 entry
   - preserves initial x0 on CPU0;
   - drops to EL1 when entered above EL1;
   - initializes per-CPU boot stacks and exception state;
   - calls kernel_main(handoff_ptr).

3. Kernel validation and seeding
   - validates x0, then fixed HANDOFF_ADDR fallback;
   - combines boot-supplied entropy or RNDR into the AArch64 PAC activation
     policy;
   - initializes the heap and frame allocator from handoff ranges;
   - reserves kernel/heap, BootFS, update/snapshot, and platform reserved ranges;
   - parses BootFS and launches appd from APPD_PATH.
```

## BootFS And `appd`

BootFS is an external read-only image, not a kernel-embedded initrd. The kernel
maps the verified `bootfs_addr..bootfs_addr + bootfs_len` range as an immutable
VMO, parses the BootFS directory table, and locates `APPD_PATH`:

```text
/boot/pkg/bexos.platform.appd/bin/appd
```

The kernel loads that ELF into an isolated EL0 address space, creates the initial
bootstrap channel, sends the BootFS VMO handle and `/boot` namespace metadata,
then enters userspace. Appd imports BootFS packages, starts wave-ordered early
drivers and services, pivots to persistent storage, and releases BootFS pages
once `/boot` is no longer needed.

## Update Reservation

The cold-boot handoff reserves `UPDATE_BASE..UPDATE_SNAPSHOT_END` for
heart-transplant kernel replacement and snapshot data. The frame allocator keeps
this range unavailable during normal allocation. The replacement kernel itself
is separately linked at `UPDATE_BASE`; snapshot records begin at
`UPDATE_SNAPSHOT`.

Future production boot flows may derive the same descriptor from FDT/ACPI,
Secure Boot metadata, or A/B boot selection state. Those flows should still use
the versioned `BootHandoff` shape instead of passing Rust-owned structures or
bootloader-private pointers.
