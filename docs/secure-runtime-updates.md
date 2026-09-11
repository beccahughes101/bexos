# Secure Runtime And Updates

## Current secure stack

The complete x86 monitor-replacement and firmware-recovery plan is not yet accepted.
The current x86 checkpoints are described in
[secure integration validation](secure-integration-validation.md). They include
actual BexOS-to-Trusty transport, confined device DMA and two authenticated
product boots with persistent storage. Actual product monitor policy replacement
has executed and committed generations 2 and 3 with retained secure clients,
fault/hang rollback and old-memory reuse. The revised policy passed monitor
reboot and persistent-data continuity. The authenticated-selection policy
correction also passed actual Trusty generation-2 reboot and committed-image
corruption recovery. The complete final E2E matrices still require validation.
Live Trusty replacement and ARM
execution tests are excluded from this x86 effort.

The product now uses a permanent recovery/execution nucleus, with separately
signed replaceable scheduling code, private data and stacks. CPU, device,
interrupt, timer, DMA and transport ownership remains resident. A dedicated ATA
firmware disk provides bounded component A/B slots. Slot bytes are flushed,
reread and authenticated before protected pending publication; a separate
recovery Trusty instance resolves boot selection and is discarded before normal
clients start. Cold trials forward complete RPMB reads while blocking writes
and key programming. The expanded native Trusty recovery suite passed with
persistent-data checks and interruption of writes, publication, trials and
acknowledgements; see the validation record for its exact tested scope.

The x86 prototxt policy explicitly permits authenticated firmware selection.
After protected selection and generation approval, the nucleus seals a boot
evidence flag alongside the actual selected Trusty measurement. The kernel
confirms that private seal before appd accepts the selected measurement. Appd
requires the policy opt-in, monitor evidence version, secure-boot and RPMB flags;
otherwise the configured bootstrap image hash remains required. The option is
disabled by default and does not alter another platform's policy.

`updated`, `teed`, the provider, debugd and bexctl expose explicit live/on-reboot
activation and staged, pending, committed, rolled-back and recovery-required
outcomes. Trusty accepts reboot activation only. Component updates serialize,
and their protected generation floors are independent of application updates.
The implementation checkpoints below describe the earlier reconstruction work;
they do not supersede the current product status above.

ARM's authenticated BL33 now obtains actual Trusty AVB/RPMB rollback approval
before entering the kernel. The security-stack regression passed with that
firmware, including reboot persistence, an independently signed rollback-floor
provisioner and explicit stale-generation/corrupted-RPMB rejection. The normal
RPMB proxy takes over the same helper at a drained frame boundary. A separate
ARM AuthMgr readiness failure exposed lost secure timer delivery; scoped GIC
interrupt ownership and its kernel-transplant state are implemented in source,
with guest validation pending. See the validation record for exact bundles.

The QEMU firmware is built from the pinned Trusty superproject by Bazel and
contains upstream KeyMint, Gatekeeper, secure storage, AVB, AuthMgr FE/BE, plus
the BexOS orchestrator and required upstream platform providers (hwcrypto,
hwbcc, hwcryptohal, and system-state). The removed custom KeyVault and TUI are not part of the
build or service graph. ConfirmationUI remains a future design until secure
display/input ownership exists.

`teed` is a provider-neutral, heart-transplant-capable service. Product
prototxt selects either the Trusty ABI-v1 driver or the explicit software test
provider. The Trusty provider implements SMC/shared-memory and queued TIPC
session open/invoke/close operations. Missing ABI symbols, transport, root of
trust, or RPMB support fails closed; there is no automatic software fallback.
Raw Trusty endpoints are not published to ordinary applications.

Typed privileged clients cover KeyMint, Gatekeeper, storage, AVB, AuthMgr, and
the orchestrator. Trusty storage persists secure-service state over RPMB. On
QEMU, the harness owns the upstream `rpmb_dev` process and its `rpmb0`
virtio-serial connection, preserves the image across instance boots, and tears
the daemon down with QEMU. The QEMU root and RPMB state are development-only.
Future physical boards must satisfy the documented eMMC/UFS RPMB, authenticated
BL33, provisioned-root, and pre-kernel Trusty transport contracts.

## Identity and key authorization

`usersd` enrolls and verifies passwords with Gatekeeper and stores only the
Gatekeeper handle, secure user ID, and an opaque auth-bound KeyMint HMAC key
blob. A successful verification caches the encoded hardware-auth token for at
most 300 seconds. Lock, disable, deletion, password replacement, or expiry
invalidates it. Unlocking an already-unlocked user reauthenticates and refreshes
the token without reopening BexFS.

For each unlock, `usersd` asks KeyMint to compute HMAC-SHA256 over
`bexos.ukek.v1\0 || uid_le64`; the 32-byte result is transferred to `vfsd`, then
zeroized. Raw UKEKs are never persisted. User creation enrolls Gatekeeper,
creates the auth-bound KeyMint blob, verifies the password, initializes the
encrypted filesystem, and leaves it locked, rolling every stage back on error.

Valid cached tokens migrate through kernel-controlled `usersd` heart-transplant
state with their original secure timestamp and expiry. Migration never extends
validity. Commit retires source scheduling, capabilities, and mappings. Bounded
kernel maintenance then scrubs its private pages before returning them to the
allocator; pending pages remain kernel-owned and checkpointed. An aborted
handover resumes the unchanged source copy and retires the candidate instead.
`keychaind` obtains a token from the private capability-restricted usersd broker
for each protected operation and does not keep a second token cache.

KeyMint is authoritative for Ed25519, P-256, AES-GCM, signing, opaque key blobs,
and secret-envelope encryption. `keychaind` persists aliases,
characteristics, public material, opaque blobs, and encrypted secret payloads;
it never stores hardware private-key material. Existing software-only secret
records remain readable, while non-hardware key generation is unsupported.

Gatekeeper tokens flow directly to KeyMint through the upstream shared secure
secret. AuthMgr is used for its upstream DICE and secure-service
connection-authorization role, not as a token broker.

## Updates and heart transplant

The orchestrator retains the Trusty core/update lifecycle state model,
last-known-good metadata, A/B decisions, and heart-transplant coordination.
The Rust slot coordinator now keeps generation floors unchanged through switch
and health confirmation, advances them only after a successful storage commit
callback, and blocks rollback after an uncertain commit until recovery resolves
it. Its 30-second preparation and 150-ms service-readiness deadlines have unit
coverage. The allocation-free `//secure/orchestrator:tee_slots` library is shared
by the orchestrator and the bare-metal monitor. The x86 monitor coordinator requires a trusted image verifier and
explicit product-owner readiness, switch, and durable-commit events. Guest
activation returns Busy until those events complete; missing verification and
reboot-selection backends return Unsupported. An audit removed placeholder
hashing and simulated health/commit success. Activation also checks the configured
update-owner domain. The product monitor now installs copied candidate staging
with independent AVB authentication. Reboot activation now starts selected Trusty
images under a restricted trial, fences persistent writes, and resolves protected
commitment before normal services begin. Live Trusty startup with storage
snapshot/catch-up and retained sessions remains future work outside this scope.
`teed` preserves public session/catalog/update state while
dropping in-flight secure calls with an explicit unavailable error. Appd keeps
trusted-app package activation transactional and rolls back candidates that do
not load and probe successfully.

The shared update and TUF libraries continue to verify signed artifacts,
metadata thresholds, rotation, expiry, hashes, lengths, and rollback floors.
Protected monitor checkpoint records now cover guest CPU/MSR/SSE state,
shared registrations and pending transport, virtual interrupt controllers,
original timer deadlines, halted CPUs, UART output, RTC updates and assigned
PCI software state. Import reconstructs host mapping/control pointers from
resident policy and validates canonical state before changing destinations.
Their checksums detect damage; they do not establish trusted provenance.
CPU restoration and combined platform/transport restoration have actual SVM
execution coverage, including a fetched Trusty IPC request. Four-CPU BexOS
aggregate validation and reconstruction have a passing guest probe, including
SMP, WASM, storage verification and memory reuse. These records do not install a new monitor or transfer physical
DMA ownership. Actual product policy replacement now uses the permanent nucleus
and authenticated commitment instead of restoring those earlier guest snapshots.

The x86 runtime now links hardware-retained page tables, VMCBs, permission
maps, VT-d tables, watchdog entry points and PCI geometry above `0x18000000`.
The retiring image's code and ordinary software state remain below that
region. Initial PCI geometry is captured in protected versioned records, so
normal-domain import can reconstruct policy without probing active BARs or
borrowing an object from the old image. Linked ELF checks reject misplaced
resources and writable executable segments. The normal-domain resume
constructor reconstructs the software owner from its protected record and
resident roots. Its expanded guest probe passed in 352.6 s after bounded
world scheduling repaired a startup timeout also present in the baseline.
Boot approvals now use protected versioned records in the resident region;
their host checks and refreshed rollback-rejection guest pass. Signed staging
also passes tamper rejection, repeated valid-image authentication, staging
reuse and continued client requests. Secure-owner import now validates CPU,
peripheral and transport records before installing live state; the expanded
atomic-rejection guest passed in 32.0 s after correcting a miscompiled aligned
stack frame, including unchanged old-owner snapshots after rejection and
continued real Trusty IPC after reconstruction. The permanent nucleus adds a
separate candidate entry/recovery interface and commitment-gated reclamation;
the reconstruction probe remains separate evidence.

The ARM QEMU GICv2 compatibility patch now has an uncached AuthMgr guest pass
(66.9 s), including accepted authenticated IPC, both rejection cases and BexOS
debug readiness. It selects the implemented main interrupt interface with
secure AckCtl because QEMU lacks the alias registers used by pinned Trusty.
Both saved ARM bundles now include the patch. Standard security-stack and
kernel-replacement regressions passed in 133.4 s and 174.2 s respectively,
with retained client progress and a 2 ms kernel cutover.

TF-A and Trusty/LK are refreshed explicitly through the saved firmware bundle
and take effect on the next QEMU launch. Integrated x86 product builds now also
consume the saved EFI/monitor closure from `//boot/efi:cached_firmware`, refreshed
explicitly by `//boot/efi:refresh_firmware`; its verifier accepts independently
authenticated bounded kernel/BootFS snapshots and constructs its own handoff.
The runtime core-update entry point uses a dedicated pinned 64 KiB bounce buffer
to transfer full-sized images into monitor-private memory. The versioned
`BEXFW001` envelope authenticates the exact ELF, architecture, component,
state ABI and generation through signed AVB descriptors. Trusty and hypervisor
artifacts use separate rollback locations (30 and 31); staging does not advance
or approve a durable floor. The root enforces a 30-second preparation budget,
freezes the completed snapshot and erases it on revocation, abort, failed
authentication or expiry. `teed` accepts up to 64 MiB and reports protected
activation outcomes separately from transport failures. The product controller
writes authenticated durable slots and cold-boots selected Trusty firmware.
Update/TUF/FIDL interfaces identify the x86 hypervisor as a distinct
component, which cannot enter the kernel transplant loader. The versioned
dual-slot capsule and retained orchestrator state remain the contract for a
future board implementation; QEMU does not claim a live physical secure-world
replacement.

Integrated firmware cache format 3 includes independently signed
`trusty.replacement.fw` and `hypervisor.replacement.fw` artifacts.
`//boot/efi:cached_replacement_authentication_test` verifies them with the same
allocation-free parser used by the monitor. The bundle includes distinct monitor
policy candidates, fault/hang fixtures, a generation-3 successor and later fault
trial, and a separately built generation-2 Trusty image. Standard and acceptance
bundles require explicit sequential refreshes after firmware changes.

## Current limits

- Physical-board secure boot, RPMB, and Trusty replacement have not been
  validated because the repository has no physical board target.
- QEMU development roots and RPMB images are not production provisioning.
- Secure ConfirmationUI and secure display/input ownership are unimplemented.
- D2 driver lifecycle and physical DMA ownership transfer remain future work.

### Debug terminal continuity

Debugd's current migration record includes an active terminal's identity, provider
name, control/stream handles, session counter, and stdin EOF state. Socket queues
remain kernel resources and transfer with their handles; quiescence occurs between
bounded exchanges. The lease is renewed on activation and paused during quiescence.
The updated proxy channel and in-progress update upload streams also participate
in the record. Older records without terminal state restore with no active shell.
Version 10 additionally preserves whether the user-service channel has an
outstanding reply after a timeout. The next call drains that reply before
issuing another untagged request, including after transplant. User mutations
have a bounded 360-second proxy budget for nested durable filesystem work;
this does not extend authentication-token lifetime or replacement deadlines.
Partial credential-bearing requests are not exported; their snapshot attempt is
aborted while the source remains available to complete authentication.


## Earlier root commitment and reconstruction checkpoint (2026-09-07)

This checkpoint preceded the resident product implementation summarized above.

The in-tree Trusty orchestrator now has a separate kernel-UUID-only replacement
journal endpoint in source. Its rollback-protected TP file stores a canonical
architecture/component-specific A/B record. An atomic storage transaction binds
the image hash and slot to the new generation; exact retries are idempotent and
uncertain replies require a fresh authenticated query. X86's reserved root
transport operation reaches this endpoint through kernel IPC without changing
normal QL client UUIDs. The product boot owner combines journal generations with
AVB floors before allowing execution. Current signed base images still have
generation one; a committed later slot must be rejected until a corresponding
boot selection implementation can load its authenticated image. The refreshed
x86 journal passed actual guest commit, reopened read, idempotence, conflict and
reboot-rejection checks on 2026-09-07. Cold TP startup uses a shared 30-second
request budget; secure cutover remains 150 ms. Live replacement does not yet
invoke durable commitment.

The saved-Trusty checkpoint guest passed after discarding/reconstructing CPU,
virtual platform and root transport objects, including a fetched, uncompleted
request. Registrations, ticket identity, completion buffer and evidence travel
in a validated protected record. This does not transfer physical DMA tables or
enter a different monitor image. Resident nucleus, activation, write fencing,
provider reconnection and old-image reclamation remain implementation work.

The ARM wakeup path is being corrected to forward per-CPU secure timer/IPI
notifications. The initial early-boot allocation bug is fixed. The latest
kernel transplant passes, but security stack and AuthMgr failed when a standard
call repeatedly yielded with timer 29 pending. QEMU's GICv2 lacks the aliased
registers selected by pinned Trusty. An ARM QEMU-only patch uses the main
registers with secure AckCtl and leaves Group 0 delivery on FIQ; its firmware
refresh and guest checks are pending. See the validation log for exact results.
The protocol reference is Google's
[Trusty IRQ driver](https://android.googlesource.com/kernel/google-modules/trusty/+/2494a3e105ef21fd204c44598e1a960f73400b90/drivers/trusty/trusty-irq.c).
