# RFC 0022: Update engine

- Created: 2026-08-28T13:48:23-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

Updated coordinates package and platform updates using signed metadata, appd lifecycle operations, and heart-transplant paths. The design records current verification and recovery behavior alongside future production update support.

## Design overview

BexOS uses an in-repo `updated` path for QEMU app/package and platform
updates. `//lib/tuf` verifies TUF repository metadata, while `//lib/update`
continues to verify direct debug-upload manifests. Ordinary app updates remain
install-and-launch operations; protected core-service bundles use appd's
userspace heart-transplant coordinator. Platform updates use the kernel
heart-transplant path for microkernel images and the D1 TEE manager for signed
TEE core images.

## Architecture

```
+-------------------------------------------------------------------------+
| update verifiers (`lib/tuf`, `lib/update`)                         |
| - verifies TUF repository metadata and direct BexOS manifests             |
| - verifies generation, size, target hashes, and Ed25519 signatures        |
+----------------------------------+--------------------------------------+
                                   |
             +---------------------+---------------------+
             |                                           |
             v                                           v
+---------------------------+             +-------------------------------+
| app package update        |             | platform update               |
| debugd -> appd     |             | debugd -> kernel or teed      |
| existing signed .bex path |             | orchestrator heart transplant |
+---------------------------+             +-------------------------------+
```

`services/updated` is packaged as `bexos.service.updated` in QEMU
BootFS and announces readiness as the system update component. The QEMU debug
transport still enters through `debugd`, which proxies update upload, apply, and
status calls to `updated` over `bexos.update.UpdateManager`.
Current feed support verifies TUF metadata with `//lib/tuf`, serializes trusted
root/rollback-version state for heart transplant, stages target bytes only
after exact target hash/length checks, and passes verified app package bytes
directly to appd. The shared distribution client now keeps the TUF/app-update
policy core separate from the buildable `//lib/distribution:distribution_live`
Netstack/rustls HTTPS transport. The live target is now a thin compatibility
wrapper around `//lib/net:net_secure`, which resolves hosts through Netstack,
connects TCP socket streams, imports trustd's TLS REDB root bundle into rustls,
enforces hostname/SNI verification, and parses bounded HTTP/1.1 content-length
and chunked responses. Injected transports remain test-only for deterministic
TUF verification coverage; QEMU product images still link the core client
unless they explicitly select live app/update fetching.

## Signed Manifest

The update manifest is a compact deterministic binary record generated and
consumed by BexOS code. Config sources and app manifests remain prototxt, but
the runtime update manifest avoids pulling protobuf or TUF machinery into the
no-std path.

Fields:

- `generation`: monotonic generation; values below the accepted floor are
  rejected as rollback attempts.
- `target_id`: package id or platform target name.
- `artifact_kind`: `APP_PACKAGE`, `MICROKERNEL`, or `TEE_IMAGE`.
- `artifact_len`: exact artifact byte length.
- `artifact_hash`: BLAKE3 hash of the artifact bytes.
- `key_id` and `signature`: Ed25519 signature over the unsigned manifest
  payload.

Verification rejects bad magic/version, unsupported artifact kinds, empty or
oversized artifacts, length mismatch, hash mismatch, unknown keys, bad
signatures, and rollback generations.

## Debug Commands

The host streams a manifest and artifact to `debugd`, then invokes a narrow
debug command:

```sh
bazel run //tools/bexctl:bexctl -- --socket /path/to/debugd.sock update check [PACKAGE|TEE|KERNEL] [--all] [--stage] [--apply]
bazel run //tools/bexctl:bexctl -- --socket /path/to/debugd.sock update app MANIFEST BUNDLE.bex
bazel run //tools/bexctl:bexctl -- --socket /path/to/debugd.sock update service MANIFEST BUNDLE.bex
bazel run //tools/bexctl:bexctl -- --socket /path/to/debugd.sock update platform MANIFEST ARTIFACT
bazel run //tools/bexctl:bexctl -- --socket /path/to/debugd.sock tee update-core MANIFEST TEE_IMAGE
bazel run //tools/bexctl:bexctl -- --socket /path/to/debugd.sock exec update.status
bazel run //tools/bexctl:bexctl -- --socket /path/to/debugd.sock exec kernel.update.status
bazel run //tools/bexctl:bexctl -- --socket /path/to/debugd.sock tee update-status
```

The underlying `ExecCommand` allowlist remains narrow. It supports named
diagnostic/update commands such as `debugd.version`, `kernel.ps`,
`update.apply_app`, `update.apply_platform`, and status queries. Arbitrary shell
commands remain unsupported.

TEE management also has typed debug-wire methods and `bexctl tee` commands for
info, trusted-app list/install/uninstall, session open/close, command invoke,
TEE core update, and TEE update status. The direct TEE management methods are
developer/debug operations; `bexctl tee update-core MANIFEST TEE_IMAGE` uses
the same signed update upload path as `update platform` before applying the
staged TEE image.

## App Updates

App package updates use the existing signed `.bex` archive pipeline:

1. Host uploads the signed update manifest and `.bex` artifact through debugd.
2. `lib/update` verifies the manifest and artifact inside `updated`.
3. `updated` forwards the verified `.bex` bytes to appd through the
   existing app lifecycle install path.
4. appd verifies the archive signature, writes it through vfsd, and
   updates the registry.

Ordinary applications are not silently upgraded by the service protocol; their
existing install behavior is unchanged. For protected core services,
`update.apply_service` returns when staging begins and
`update.service.status PACKAGE_ID` reports preparation, live bulk, catch-up,
cutover, committed generation, or rollback. Active selection and accepted
generation change only at commit. The current selected service archive is held
in memory and is not yet persisted across reboot.

## Platform Updates

Microkernel updates use the kernel debug protocol as a QEMU control bridge.
`//kernel:update_kernel` builds the complete kernel separately at `0x42000000`,
with `kernel_transplant_entry` as its ELF entry instead of cold boot:

1. Host uploads a signed platform update manifest and artifact through debugd.
2. `lib/update` verifies generation, hash, size, and signature inside `updated`.
3. `updated` invokes `KernelDebugControl.ApplyPlatformUpdate` with a readable
   artifact VMO as well as kind, generation, target, length, and hash.
4. The kernel checks debugd privilege, generation, target, BLAKE3, ELF load
   ranges and executable entry, then copies segments into reserved RAM.
5. After replying to the host, `updated` triggers preparation. CPU0 copies bounded
   typed runtime and allocator records between scheduling opportunities;
   mutation tracking supplies ordered catch-up records while userspace runs.
   The replacement preparation entry uses its own stack and heap without taking
   hardware ownership.
6. CPU0 quiesces after catch-up, captures final contexts/deltas, validates
   receipts and authorizes SWITCH. Execution branches into the uploaded kernel,
   whose runtime metadata is already reconstructed, and resumes the interrupted
   EL0 task with the original GPR/SIMD context and page tables.
7. The replacement marks completion and reclaims the old image, stacks and
   heap. An allocation/write/free probe proves reuse of an old-kernel page.

The AArch64 cold-boot allocator reserves `0x42000000..0x43000000` for the replacement
and heap and `0x43000000..0x43400000` for snapshots. Secondary CPUs acknowledge
parking in a reserved trampoline outside either kernel before staging is allowed.
The handoff lives at `0x40101000`; the secondary park trampoline at `0x40102000`.
Q35 uses the architecture-selected reserved ranges in `//lib/boot`, with its
handoff and parking area starting at `0x01000000` and replacement slot at
`0x10000000`. Both paths retain the CPU0 takeover ownership restrictions.

This implements one CPU0 takeover, not repeated in-place upgrades of the same
staging slot. A second replacement overlapping the live kernel is rejected.
TEE images remain a distinct artifact kind and are never executed as kernel
ELFs. After manifest verification, `debugd` routes `TEE_IMAGE` artifacts to
`teed` through `bexos.tee.TeeManager.UpdateTeeCore`; `MICROKERNEL` artifacts
continue to use kernel debug FIDL. The current QEMU implementation models TEE
core completion in D1 and advances the platform generation only after the TEE
backend reports completion. Physical-board Trusty/RPMB validation, persistent
A/B writes, SMP ownership migration, and rollback after a fault inside
replacement code remain future work. QEMU preserves existing userspace physical VMOs, page tables and
DMA memory. Core services migrate separately before the kernel; this is not a
whole-system atomic transaction.

## Tests

The milestone is covered by:

- `//lib/update:update_tests`
- `//lib/debug_wire:debug_wire_tests`
- `//services/debugd:debugd_tests`
- `//services/teed:teed_tests`
- `//kernel/core:core_tests`
- `//testing/e2e/qemu/update:app_package_update_e2e_test`
- `//testing/e2e/qemu/update:tee_image_update_e2e_test`
- `//testing/e2e/qemu/update:kernel_platform_update_e2e_test`
- `//testing/e2e/qemu/update:all`

The QEMU e2e targets install a signed app update, confirm protected app policy
still holds, verify `bexctl tee` info/app/session/invoke flows through debugd
and teed, reject a bad signed TEE update before it reaches teed, apply a signed
TEE image through the TEE manager, split service replacement coverage by updated
component, apply a signed microkernel platform update, check debugd health after
the transplant trigger, query kernel update status, and reject a rollback
generation.

## Future Design

The production update system can still grow into the broader architecture that
motivated this milestone:

- Resumable HTTPS/connectRPC download, staging files on RoseFS, and BLAKE3
  chunk/Merkle verification can replace the current debugd upload transport.
- Userspace migration can expand from the protected core set to other runtimes
  and applications that implement the explicit record protocol.
- Future Trusty production integration can authenticate staged trusted-app
  bundles, persist AVB anti-rollback counters in board RPMB, arbitrate secure
  world CPU cutover, and sync verified kernel/TEE images to inactive A/B boot
  slots after a successful live transplant.
