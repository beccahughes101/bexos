# Complete Trusty replacement on maintained QEMU products

Approved design, 2026-09-09. This specifies the intended completed system,
**not implemented or accepted behavior**. The
[current checkpoint](../current/trusty-completion.md) records what exists.
Physical boards, production provisioning, ConfirmationUI and additional
speculative secure services remain future work.

## Execution owners

Replace the complete Trusty/LK payload and its TAs. TF-A and permanent execution
owners update through authenticated reboot firmware. Preserve the independent
x86 monitor-policy replacement boundary.

Use a shared coordinator with separate architecture adapters, protected state
codecs, storage migration and transport reconnection modules. Owners must
observe execution, isolation, storage readiness and service health themselves;
candidate assertions do not establish these properties.

On x86, the permanent nucleus owns independent active/candidate memory banks,
nested mappings, CPU contexts, controllers, timers and transport lanes. Neither
guest can map its peer, the nucleus, migration storage or candidate-upload
memory. Candidate faults and watchdog expiry return control to the nucleus
without stopping the source or normal-world processes.

On ARM, a signed permanent S-EL2 owner runs beneath TF-A and hosts Trusty at
S-EL1. It owns secure stage-two mappings, interrupts, timers, watchdogs,
shared-memory access and replacement. The pinned QEMU profile must implement
secure virtualization. TF-A context handling and SMP entry must preserve
ownership through world switches. BexOS retains existing service interfaces and
the guest-visible interrupt contract.

Stock QEMU's 16 MiB secure RAM is insufficient. A pinned Bazel-built extension
adds secure-only transition memory. Prototxt generates matching QEMU, firmware
and linker definitions with explicit owner, two-bank, upload and migration
reservations. Bounds and overlap validation are build gates. A platform probe
does not substitute for product stage-two peer-isolation tests.

Firmware control uses kernel-authorized pinned buffers on both architectures.
Guest-supplied addresses, identities and migration records cannot grant resident
authority.

## Preparation and continuity

Keep the 30-second preparation deadline. Authenticate immutable bytes,
architecture, component, generation and migration compatibility. Persist and
reread the inactive slot before publishing protected pending selection or
executing the candidate.

Run the candidate while the source serves clients. Transfer a consistent
secure-storage snapshot and a bounded stream of committed changes through
protected channels. Missing deltas, stream exhaustion and incompatible state
abort preparation.

The resident owner enforces one persistent writer. Candidate preparation uses
an isolated storage view; it cannot modify authoritative RPMB, program keys or
advance rollback floors. Lost write acknowledgements fence further writes
until authenticated observation resolves their outcomes. Never automatically
replay mutations whose outcomes are uncertain.

Versioned TA/provider hooks preserve KeyMint/Gatekeeper authentication secrets,
boot authorization, throttling, secure-clock continuity, lifecycle state,
persistent keys and opaque blobs. Preserve unexpired tokens without extending
their expiry. Reseed candidate randomness independently. AuthMgr must authorize
the actual candidate measurement: retained principals must not present the old
image measurement as the new image identity.

## Cutover and persistence

Start the 150-ms cutover/readiness clock **before quiescence**. Drain calls or
terminate them with explicit retryable errors, fence source writes, apply the
final state/storage delta and switch routing. Preserve public session IDs
while rebinding internal handles. Transport generations reject stale replies
and operation handles. Probe all required secure services before commitment.

Bind the image, generation and storage ownership transition in authenticated
persistent state. Resolve lost commit acknowledgements by authenticated
readback. Resume the source only after noncommit is established; unresolved
commitment remains recovery-required with both writers fenced.

After commit, admit candidate writes, publish its actual running measurement
and generation, and scrub retired private memory before reuse. Before commit,
faults, hangs, incompatible state and exceeded deadlines restore the source.
Retain on-reboot activation. Both architectures require durable A/B selection
and authenticated pre-kernel recovery that preserves acknowledged data and
never selects firmware below the committed floor.

## Protocol, artifacts and acceptance

Internal control/migration protocols describe capabilities, compatibility,
transport generations and coherent transaction status. Reuse public live and
on-reboot modes and existing outcomes. Live Trusty requires the installed
execution owner's explicit capability; unsupported firmware fails closed.

Build independently signed generation-2 and generation-3 Trusty images,
incompatible-state candidates and fault/hang fixtures on both architectures.
Include them in versioned standard and acceptance bundles. All builds/codegen
run through Bazel; configuration is prototxt and generated artifacts remain
uncommitted. Refresh saved closures sequentially.

Required completion evidence:

- Protocol, authorization, migration compatibility, snapshot consistency,
  fencing, stale-reply, deadline, commitment, rollback and reclamation tests.
- Actual 1→2→3 product replacements on both architectures, observing distinct
  candidate execution, unchanged normal-world processes, retained sessions,
  continued progress and measured deadlines.
- KeyMint algorithms/blobs, Gatekeeper password/throttling flows, token
  validity/expiry, encrypted filesystems, persistent storage, AVB negatives,
  AuthMgr acceptance/rejection and orchestrator reachability across replacement.
- Tampered/wrong-architecture/wrong-component/stale images, malformed state,
  candidate faults/hangs, concurrent writes, interrupted transport and attempted
  cross-domain memory access.
- Reboots after interruptions during slot writes, pending publication, cutover,
  commitment and acknowledgements, checking corruption handling, authenticated
  selection and preservation of acknowledged data.
- Service/kernel transplants before and after Trusty replacement, plus existing
  x86 monitor replacement.
- Bazel Rust formatting, both host suites, optimized image checks, matrix
  membership and uncached maintained ARM, integrated-x86, x86-development and
  firmware-acceptance matrices. Reap all task-owned QEMU/RPMB processes.

Record exact commands, results and consumed artifact hashes in current docs.
Missing or failed gates prevent complete-acceptance claims. Historical
checkpoints, models, standalone boot and platform probes cannot substitute for
final-tree product evidence.
