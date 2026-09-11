# RFC 0004: Userspace heart transplant

- Created: 2026-08-26T17:31:23-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

Core services update independently through explicit lifecycle records, resource adoption, and rollback. The kernel serializes transactions; replacement is not a whole-system atomic update.

## Design overview

Core services use the same `lib/migration` lifecycle and record primitives as
kernel live migration. Each component updates independently; the kernel serializes
transactions. This is not a whole-system atomic update.

## Manifest configuration

Sources are `.prototxt`; Bazel generates binary manifests. Omitted lifecycle
configuration retains `RESTART`. Opted-in services never silently restart after
migration failure:

```protobuf
lifecycle {
  update_strategy: HEART_TRANSPLANT
  migration_timeout_ms: 150
  preparation_timeout_ms: 30000
}
```

Both old and replacement process manifests must opt in. Appd has its own package
`bexos.platform.appd`, with no cold-boot wave declaration. Its cold ELF is
`/boot/pkg/bexos.platform.appd/bin/appd`.

## Protocol and isolation

The source's dedicated `Migratable` channel supports Prepare, NextBulk, NextDelta,
Quiesce, Abort and GetStatus. A private candidate `StateReceiver` channel supports
Initialize, AdoptBulk, CompleteBulk, AdoptDelta, Validate, Activate and Abort.
The authoritative schema is `idl/bexos/app/migration.fidl`. Responses carry the
transaction generation and method ordinal. The declared CommitAndExit operation
is reserved; retirement is performed by the kernel transaction, not an exit RPC.

Appd verifies the platform-signed archive, package identity, generation, manifest
strategy and ELF before launching a quarantined candidate. Bulk records use stable
keys and checksums; subsequent ordered deltas carry mutations and tombstones.
The candidate rejects incompatible versions, missing/extra keys, malformed records,
sequence gaps and capacity violations. Payloads are bounded to 32,704 bytes,
aggregate retained state to 8 MiB, and dirty keys to 16,384. Large byte arrays use
16 KiB chunks. A service's adapter must record every relevant mutation.
The kernel runtime state format is versioned separately from service records:
current full and incremental runtime records preserve VMO backing kind, lazy
per-page backing state, the shared zero page, and per-process userspace PAC key
material. Older contiguous-VMO runtime snapshots remain restorable as contiguous
backing, while new first-page commitments and DMA materialization mark the VMO
dirty so live deltas carry the change.

Normal request dispatch continues during copying. At quiescence, dispatch stops,
remaining dirty records are drained, resources are validated and the source queues
Validate/Activate directly to the candidate. This lets the candidate complete even
when appd is waiting on a normal service RPC, and lets appd replace itself without
waiting on its own suspended coordinator. Existing endpoints retain their messages.

`bexos.kernel.migration` owns the resource transaction. Before commit the candidate
cannot consume production channels, map transferred shared resources, use MMIO,
pin DMA memory or use transferred authority. The kernel atomically changes the
owners of original endpoints/capabilities/pins, installs preserved mappings and
transfers authority. App-manager and platform-update authority are explicit process
attributes, not process-slot or package-name privileges. Abort resumes the source
and retires candidate resources. Commit retires the source, releasing private
mappings and memory. Unaffected processes retain their PIDs.

## Component adapters

| Component | Preserved state / skipped initialization |
|---|---|
| appd | Registry/protection/manifests, policy, broker/devices, boot management records, launches, package selection and pending update. Skips boot waves, mounts and SYS_STATE generation changes. |
| debugd | UART mapping, partial input/uploads, verifier floor, lifecycle proxy and update status. Uses the same host socket; output frames complete before quiescence. |
| vfsd | Initialized package store, mounted directory capabilities and routing. No remount. |
| BexFS | Volume/partition metadata, namespace and file chunks/deltas, endpoints, rights/cursors and block registrations. Keys travel in protected VMOs, not record payloads; temporary key objects zeroize on drop. |
| ArchiveFS | Mounted contents, stable nodes, endpoints and cursors; no reread/remount. |
| NVMe | Controller configuration, queue heads/tails/phases/CIDs, physical DMA mappings/pins, registered buffers and FIFOs. Awaited hardware commands finish before migration polling; adoption does not reset/recreate queues. |
| VirtIO-Net | Control and migration channels, Ethernet and power endpoints, FIFOs, registered shared VMOs, HAL DMA pins, MMIO/ECAM mappings, MAC/health/running state, and adopted VirtIO transport/queue state. Packet work drains at quiescence; adoption does not reset the NIC or rerun queue configuration. |
| I2C / SPI | Static topology, endpoint identity, registered device nodes, queued transfers, pending replies, deterministic backend contents, SPI configuration, fixture clients, and lock leases with absolute deadlines. Drain stops new work, then finishes or aborts the active bounded transfer before quiescence. |
| PCI / PL011 | Control/registration state and retained mappings; no enumeration/BAR programming/UART initialization. |
| updated | Control/migration state through the same adapter despite its small loop. |

## Update selection and recovery

`update.apply_service` applies the last verified uploaded service bundle;
`update.service.status PACKAGE_ID` reports phase, pending state and committed
generation. Staging does not change active selection or accepted generation.
Appd stages the signed archive under a content-addressed package-store archive
ID before launching the candidate and retains the candidate registry record
until commit. Abort deletes the staged archive and discards the pending record.

**Current QEMU implementation:** selected service archives and accepted
generations are durably committed in the app registry; older checkpoint records
default the accepted generation to zero. Normal app install remains a separate
disk-backed path. Existing process-capacity limits apply before staging,
including exited process metadata slots.

## Validation and current limitations

Host tests cover lifecycle ordering, deadlines/rollback, bounded transport and
mutation equivalence, runtime/resource transactions, BexFS writes/truncation/cursors,
ArchiveFS cursor state, and partial debugd upload restoration.

The update QEMU test uses separately linked replacement archives for all nine
components followed by a separately linked kernel. Its persistent client keeps
BexFS and ArchiveFS endpoints open, writes sequence values, checks split reads
and file positions, and continuously submits reads through an original NVMe FIFO
and registered DMA buffer. NVMe activation reports its adopted queue physical
addresses so the test compares them with the pre-update addresses. Preparation
rejection, incompatible state, candidate exit and timeout fixtures exercise
rollback before a valid retry. A deliberately partial test-app upload resumes
after debugd replacement on the same host connection. The test also checks
unaffected process identities.

Acceptance is not implied by the presence of a test: the complete expanded suite
must pass before claiming all-component continuity. The test requires client
counter increases while every component reports live bulk, records each measured
cutover, and rejects values beyond 150 ms. Signed invalid-kernel staging rejection
and a subsequent valid retry are covered. Faults and timeouts inside the candidate
kernel's preparation callback still require dedicated end-to-end artifacts;
service-side precommit fault coverage does not substitute for that. No sub-20/sub-50
ms latency claim is made.
Postcommit failures are outside the recovery boundary.

## Future design retained

General opt-in applications (including other runtimes), parallel preparation,
SMP, production Secure World authorization and durable anti-rollback counters,
repeated kernel replacements, whole-system atomic transactions and recovery after
commit remain future work. The original direction of zero-disconnect endpoint and
DMA handover is implemented through kernel ownership transactions rather than
repointing directories or passing raw Rust objects.
