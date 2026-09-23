# Heart Transplant

The [approved Trusty replacement design](design/trusty-live-replacement.md)
defines the permanent ARM/x86 execution owners and secure-service continuity
requirements. Its [current checkpoint](current/trusty-completion.md) tracks
implementation and acceptance separately.

The QEMU implementation uses the shared `lib/migration` lifecycle:
**stage → live bulk sync → catch up → quiesce → validate → commit → reclaim**.
Preparation and precommit failures abort to the old owner. Recovery after
ownership commits uses the last durably accepted registry record or secure slot
metadata; updates that ran but did not commit durably are not reported complete
and are not selected after reboot.

## Shared protocol

The `no_std` library provides phase ordering, deadlines, bounded dirty sets,
ordered records, checksums, and canonical codecs. Services use
`bexos.app.migration` on a dedicated startup channel; the kernel adapter uses
replacement preparation calls. Records contain versioned fixed-width values,
stable IDs, length-delimited bytes, and resource descriptors. They do not adopt
Rust object layouts, allocator pointers, or old executable addresses.

Defaults are **30 seconds preparation** and **150 ms cutover**. These are
abort deadlines, not claims about measured end-to-end latency. Only one
userspace or kernel transaction may be active. See
[userspace migration](rfcs/0004/README.md) for manifest opt-in,
service state and authority transfer.

Appd drains bounded batches of available coordinator steps, including self
replacement requests. Sources copy live catch-up batches between normal service
dispatch opportunities and leave at most one 32 KiB record for the frozen tail.
The cutover deadline is unchanged.

Userspace service replacements are staged under immutable content-addressed VFS
archive IDs before the candidate is launched. Appd keeps the old registry
selection authoritative until migration activation succeeds and the durable
registry transaction commits the selected archive path, manifest metadata,
content hash, and accepted generation together. BootFS/system manifests are
baselines at boot; verified durable selections win once persistent storage is
available.

The USB host stack participates in the same record-based lifecycle. `xhcid`
retains controller MMIO, interrupt/IOMMU handles, command-ring/event-ring/
scratchpad DMA mappings and DMA tokens, registered buffers, bus channels, queued
transfer IDs, and completion subscribers. `usbd`
retains topology generations, policy decisions, interface ownership, topology
subscriptions, and scoped interface channels. USB HID retains held input state
and pauses report submission during quiescence; USB BOT retains block buffers,
FIFO endpoints, sense/stall/reset state, queued request IDs, and the completed
write watermark so a completed write is never replayed after rollback.

The current I2C and SPI services also participate in service heart transplant.
`i2cd` and `spid` retain static topology, scoped endpoint identity, registered
device-node state, queued transfers, pending responses, deterministic backend
contents, SPI configuration, fixture-control clients, and lock leases with
absolute deadlines. During drain they stop starting new work and either complete
or abort the active bounded transfer before quiescence; adoption skips cold
registration and keeps existing endpoints attached. See
[current I2C and SPI services](i2c_spi.md) for implementation limits and
validation status.

The [Trusty completion checkpoint](current/trusty-completion.md) records the
shared live coordinator, writer gate and protected codecs now in source. They
require product execution and storage adapters before either architecture can
claim a live Trusty/LK replacement. The unchanged 30-second preparation and
150-ms readiness deadlines are abort limits, not measured product results.

The QEMU AArch64 TEE path retains the versioned dual-slot Trusty core lifecycle
model and the orchestrator TA used to coordinate it. `teed` migrates its
provider-neutral catalog, sessions, slot metadata, and generation floor; the
orchestrator remains reachable before and after supported service handovers.
Trusty security-service persistence is separate and uses upstream secure
storage backed by the lifecycle-owned `rpmb_dev` proxy and its authenticated
development image. Dynamic replacement of the running QEMU Trusty/LK firmware
is not presented as a physical-board update mechanism.

## CPU0 kernel migration

1. Verify the signed update, target, generation, ELF ranges, handoff ABI and
   replacement preparation ABI. Load the separately linked `//kernel:update_kernel`
   into the reserved replacement slot.
2. Call its preparation entry with its own stack and heap. A return trampoline
   preserves the old execution context. Preparation faults and the per-call
   watchdog return to the old kernel; the candidate does not take hardware ownership.
3. Between normal CPU0 scheduling opportunities, copy at most eight runtime or
   allocator records per slice. Processes/contexts/mappings, VMOs, capability
   slots, channel queues and queued handles, pins, authority, and allocator bitmap
   words have mutation tracking. The candidate incrementally constructs its own
   runtime metadata while userspace continues running.
4. Catch up using at most 32 coalesced dirty records per slice. Final contexts
   are tracked by the same mechanism. Quiesce only after catching up, then
   validate the prepared runtime and seal bulk/final receipts. Do not repeat
   full serialization or reconstruction in the frozen window.
5. Authorize SWITCH, enter the replacement, restore system state and resume the
   existing process context. Preserve physical userspace pages, page tables and
   DMA memory. Reclaim old image/stack/heap only after takeover; a physical-page
   allocation probe checks reuse of the old range.

Secondary CPUs now enter the normal interrupt-capable idle path after boot, and
the host-testable scheduler supports per-CPU current tasks and affinity masks.
Kernel replacement is still committed by CPU0 only: secondary CPU ownership
transfer and a parallel replacement kernel remain future work. Handoff ABI is
version **4**, full runtime snapshot version **16**, live record codec version
**2**, and preparation ABI version **2**. Architecture-bearing formats reject
cross-architecture replacement before commit; supported legacy ARM handoff and
snapshot readers remain available, including the entropy-bearing ARM v15 format.
Debugd uses snapshot v9 to combine architecture tagging with retained shell and
upload state from its ARM v6–v8 formats. Q35 uses reserved secondary parking and
descriptor-table memory outside the reclaimed image. Integrated x86 Trusty
replacement remains outside the development product.
The existing checkpoint/orchestrator model remains useful for host tests; its
phase headers are distinct from the live runtime record stream.

## Commands and checks

```sh
bazel run //tools/bexctl:bexctl -- --socket /path/to/debugd.sock update platform MANIFEST ARTIFACT
bazel run //tools/bexctl:bexctl -- --socket /path/to/debugd.sock exec kernel.update.status
bazel test //lib/migration:migration_tests //kernel/core:core_tests
bazel test //lib/app_registry:app_registry_tests //services/updated:updated_tests //services/teed:teed_tests
bazel test //:heart_transplant_coverage_test //secure/orchestrator:orchestrator_tests //tools/qemu:qemu_tests
bazel run @rules_rust//:rustfmt
```

`update.apply_platform` stages verified input; completion is reported separately
by `kernel.update.status`. `heart-transplant: live bulk sync started` and
`live bulk sync complete` bracket preparation. `precommit cutover_ms` measures
only frozen precommit work, not postcommit initialization or first client response.
Service commit logs similarly report elapsed time from cutover start; reclamation
is included in that log. Tests must check actual progress and state, not infer
continuity from a successful apply response.

## Limits and future design

There is one kernel replacement per boot into a fixed slot. Runtime records are
currently bounded to 32 KiB; an oversized process/channel record or dirty-buffer
exhaustion aborts rather than committing incomplete state. Each preparation
callback has a 100 ms watchdog within the 30-second overall preparation deadline.
These bounds can reject otherwise valid busy workloads.
The current tests and any remaining acceptance gaps are recorded with the
[userspace protocol](rfcs/0004/README.md). Do not use
`//testing/e2e/...` as the required validation for this implementation unless a
separate e2e validation pass is explicitly requested.

Future work retains alternating/repeated kernel slots, truly parallel kernel
preparation, SMP ownership transfer during replacement, physical-board RPMB
validation, future runtime adapters beyond ELF, D2 runtime support,
whole-system atomic updates, and recovery from faults outside the current
service/TEE slot metadata boundary.
