# Secure integration validation

## Stopped at user request — 2026-09-07

Work is stopped. **The approved plan has not been delivered or fully validated.**
The implementation and existing documentation/Linux submodule changes are left
in place. No further implementation or validation is scheduled by this task.

The implemented live replacement boundary is a distinct monitor scheduling-policy
image with private code, data, and stack. The permanent nucleus retains hardware
ownership and guest execution state. This must not be described as replacing all
monitor hardware-emulation code. Reboot activation and protected interrupted-update
recovery are implemented, but complete acceptance remains outstanding. Live Trusty
replacement, ARM implementation/tests, and physical boards remain excluded.

Evidence obtained before stopping:

- The complete product live/reboot/corrupt-committed-image scenario passed in
  1438.7 seconds, including distinct monitor generations, fault/hang rollback,
  retained clients and identities, old-memory reuse, monitor reboot and Trusty
  reboot. Measured monitor readiness was 82.522 and 83.983 ms. This run predates
  the latest PAT correction and saved firmware; it is not final-tree acceptance.
  Log: `/tmp/bexos-x86-selected-measurement-product-live.log`.
- All 128 selected host targets passed with `RUST_TEST_THREADS=1`, including the
  PAT correction. Log: `/tmp/bexos-x86-pat-host-kernel-serial.log`.
  The kernel E2E target in that same invocation failed a status-sampling assertion
  after actual kernel replacement executed with a 3 ms cutover.
- The checkpoint bootstrap stack correction passed its native test. Separate
  monitor and Trusty recovery suites each passed 22 boots at earlier checkpoints.
- The latest full 110-target native matrix was interrupted after **4 passes,
  2 failures, and 104 unrun targets**. Failures were the BexFS traced-boot
  180-second deadline and a service-update assertion assuming a single console
  process. Log: `/tmp/bexos-x86-native-matrices-restart.log`.

Corrections for the BexFS shared 600-second boot budget, console instance
selection, and fast kernel bulk-sync sampling are present, but their complete
native targets have not passed. The final three-target diagnostic invocation was
canceled at the user's request after 1029.374 seconds: **0 completed targets;
all three have NO STATUS**. Before cancellation, the teed prerequisite chain
verified PCI (86 ms), console (85 ms), and NVMe (91 ms) replacements and had begun
BexFS replacement. These steps are not a passing teed target. BexFS persistence
and kernel targets were still queued. Logs and build events:
`/tmp/bexos-x86-critical-chain.log` and
`/tmp/bexos-x86-critical-chain.bep.json`.

Still required for completion: resolve remaining native failures; finish the
product reboot-only scenario and all selected integrated-x86, x86-development,
firmware acceptance and standalone Trusty gates uncached against matching saved
firmware; verify the final tree's host/image/membership gates; and record complete
results and artifact hashes. No complete E2E matrix is claimed.

Last sequentially refreshed saved firmware SHA-256 values:

- Standard: `8a36bd3e2c47dfe4c3c130fedf5b33f640a392a14832ca09bcf4891ab0cf60ac`.
- Acceptance: `3d67953cf56849a43c39900aee23d34cf01b0654782deeb65fbdadd94a743c75`.

The interrupted diagnostic run's source inventory is
`/tmp/bexos-x86-critical-chain-source.json`, SHA-256
`e3158f407ffd7b85c39707dcd2a5570a2d1e5945be362e3d14d8717194a4c784`,
recorded against `d9cefe750600997af113d0bb87418f3817843608` plus its recorded
working tree. At shutdown HEAD was
`e58a14d362a3a1d3ad1f9abf0440c06bdb485ead`; the earlier results must not be
attributed to this later revision without comparison and validation.
Temporary evidence paths are local and are not committed artifacts.

The last Rust formatter invocation passed
(`/tmp/bexos-x86-instance-deadline-rustfmt.log`). Shutdown canceled the owned Bazel
test client and verified that its QEMU and RPMB helper exited; no QEMU or RPMB
helper remained in the process inventory. The Bazel server is allowed to remain.

## Earlier implementation and validation checkpoints

The approved x86 monitor replacement and firmware reboot-recovery plan is
**incomplete pending final validation**. The current implementation is on
`d9cefe750600997af113d0bb87418f3817843608` plus uncommitted changes through 2026-09-07.
The full product live/reboot scenario and 128 selected host targets have passed;
the final native matrix has not. Live Trusty replacement and ARM tests are
excluded. Historical checkpoints below retain their original revision context.

## Product activation checkpoint on `d9cefe750600997af113d0bb87418f3817843608`

The working tree now connects signed monitor and Trusty firmware uploads to the
resident controller, protected slot store, and reboot selector. Monitor accepts
live or reboot activation; Trusty accepts reboot only. Status reports carry the
protected outcome, generation, slots, and reboot requirement. Resident status
queries use a revision check to avoid mixing separate transactions. The CLI and
feed interfaces expose explicit activation. Ordinary application generation
floors remain separate from protected component generations.

The saved EFI product now uses the permanent nucleus. Its replaceable image is
the monitor scheduling policy, with private code, data, and stack; the nucleus
retains CPU, device, interrupt, DMA, timer, and transport ownership. Readiness
uses completed guest boundaries and a retained pending secure request. Candidate
faults before commitment select the retained image without restoring guest
snapshots. After commitment may have been submitted, resident execution resolves
the journal before deciding whether recovery is required.

This is still **not a complete acceptance result**. The first product native
run failed in **914.9 s**, total **1003.673 s**, after the first rollback and
retained-session checks: the test attempted a second orchestrator connection
while retaining its sole allowed client. The test now checks that retained
client plus KeyMint separately. Log: `/tmp/bexos-x86-product-live-second.log`.
The corrected run passed live fault/hang rollback, committed distinct monitor
generations 2 and 3, retained client/identity checks, and old-memory reuse.
Readiness was **78.181 ms** and **85.908 ms**. Its reboot selected the committed
image but exceeded the unchanged 600-second guest boot deadline; the target
failed in **1219.9 s**, total **1230.269 s**
(`/tmp/bexos-x86-product-live-third.log`). The candidate normal-work budget was
increased from 8–15 to 24–31 scheduling attempts per turn. The updated product
pair is pending, including queued client requests and a failed generation-3
trial returning to committed generation 2. The earlier authenticated EFI recovery
test passed in **841.0 s**, total **845.416 s**
(`/tmp/bexos-x86-product-recovery-efi-test.log`); it predates activation wiring.

The selected host invocation passed **15/15**, total **10.906 s**
(`/tmp/bexos-x86-product-selected-host.log`): firmware bundle validation and
cache boundary, saved candidate authentication, debug wire, monitor ABI,
firmware envelope/selection/store, protected recovery, Trusty driver protocol,
teed, updated, debugd, bexctl, and matrix membership. This is host evidence,
not a substitute for the remaining native matrices. Subsequent diagnostic-only
slot-write and pending-publication interruption additions are covered below.

Standard and acceptance EFI refreshes completed sequentially after formatting.
Saved bundle SHA-256 values for this checkpoint are
`088800883b33fd2dca2f44c91d91f029d546c63a2ad61e7fd04f3aa2b1a87dcf`
and `5f1d6a941b892d1240656ef0c7145098cc777203381390900fcaba5801e5dce1`,
respectively. Logs are `/tmp/bexos-x86-product-formatted-standard-refresh.log`
and `/tmp/bexos-x86-product-formatted-acceptance-refresh.log`.
Rustfmt and `git diff --check --ignore-submodules` passed before the subsequent
diagnostic additions. All ARM execution tests and live Trusty replacement are
excluded. Older checkpoints below retain their original scope and results.

The expanded recovery suites each execute **22 persistent native boots**:
`//secure/monitor:trusty_recovery_domain_boot_test` passed **264.4 s**, total
**270.130 s** (`/tmp/bexos-x86-trusty-readonly-recovery-native.log`), and
`//secure/monitor:firmware_recovery_domain_boot_test` passed **291.2 s**, total
**298.620 s** (`/tmp/bexos-x86-monitor-expanded-recovery-native.log`). New cut
points stop after a flushed partial slot write, after authenticated reread but
before pending publication, and after pending publication before acknowledgement.
The original bounded attempts, lost commit acknowledgement and missing committed
image cases remain. The Trusty trial performs a real persistent read through
the resident RPMB parser, proves a write is blocked before hardware, and confirms
the original value survives commitment and restart.

Read-fence host, protected-recovery and linked-layout tests passed **4/4**, total
**102.769 s** (`/tmp/bexos-x86-read-fence-layout-host.log`). An earlier build
failed because the UART state exceeded the recovery-ticket section; it now has
a bounded dedicated resident section checked by the linked-image test.
Fourteen additional x86 host and optimized kernel-image gates passed, total
**17.450 s** (`/tmp/bexos-x86-host-and-optimized-image-gates.log`), and eight
authority/product host gates passed, total **9.678 s**
(`/tmp/bexos-x86-authority-and-product-host-final.log`). These include both kernel
images, selected kernel core/architecture, boot/AVB/migration, retained debug
framing, QEMU process ownership, firmware signing, C protected selection, and
product service/CLI checks.

The latest standard/acceptance EFI refreshes completed sequentially in
**595.205 s** and **5.970 s**. Bundle SHA-256 values are now
`3a6426740881dc32e904d6eea1e33823a16c4a5aa49c1ee3e5dcee5ff6f704fb`
and `836400889d0362e6c72a18224ba1c38093fc453d36022a371989b538ef0874de`.
Logs: `/tmp/bexos-x86-read-fence-standard-refresh.log` and
`/tmp/bexos-x86-read-fence-acceptance-refresh.log`. Refreshed cache authentication,
dependency isolation, bundle validation and matrix membership passed **4/4**,
total **3.627 s** (`/tmp/bexos-x86-refreshed-cache-and-membership.log`).
The focused product pair failed on initial boot before replacement: live in
**371.3 s** at user-store initialization, reboot in **601.2 s** at the unchanged
600-second guest boot deadline, total **984.437 s**
(`/tmp/bexos-x86-product-live-reboot-final-focus.log`). The harness stopped on a
partial failure line; it now drains the remainder of that line for at most two
seconds so the error detail survives. Concurrent host builds were active and
QEMU CPU use was approximately 20%; an isolated rerun is required to distinguish
contention from a product regression.
The complete selected native inventory contains **110 targets**, with no ARM
target. Its build passed in **1249.217 s**
(`/tmp/bexos-x86-complete-matrix-build.log`); the final uncached matrix result
and final tested tree remain outstanding.

The rerun without this task's concurrent builds passed initial boot, live fault
and hang rollback, four retained queued requests, and distinct monitor 2/3
commitments in **33.282 ms** and **94.171 ms**. Reboot into monitor 3 passed
and prior guest data survived. Trusty 2 was staged, its fenced trial passed,
and commitment was authenticated, but appd then rejected its changed measurement
against the static installation hash. The target failed in **1149.5 s**, total
**1166.012 s** (`/tmp/bexos-x86-product-live-isolated-diagnostic.log`).

The correction adds an explicit x86 prototxt policy opt-in and a resident-sealed
authenticated-selection evidence flag. Appd accepts a changed measurement only
with both, the monitor evidence version, secure boot and RPMB protection. The
kernel's resident evidence confirmation remains required. Bootstrap pinning
remains the fallback. Three x86 policy tests passed in **1.5 s**, total
**42.812 s** (`/tmp/bexos-x86-selected-measurement-host.log`).
Standard/acceptance refreshes completed sequentially in **6.529 s** and
**7.960 s**, with bundle SHA-256 values
`baeacd0379d2157f0f2c47955b438aa6cf406e207e6408557e34e0b75d4698d0`
and `a7d53265b73f187b55840aa9cdec98729ff854d8fd4d505479cb97fd2f9cb616`.
Logs: `/tmp/bexos-x86-selected-measurement-standard-refresh.log` and
`/tmp/bexos-x86-selected-measurement-acceptance-refresh.log`.

The corrected complete product live scenario passed uncached:
`//testing/e2e/qemu/update:firmware_live_e2e_test_x86_64`, **1438.7 s**, total
**1452.921 s** (`/tmp/bexos-x86-selected-measurement-product-live.log`, build
events `/tmp/bexos-x86-selected-measurement-product.bep.json`). It proves live
fault/hang rollback, four retained queued requests, guest identities, monitor
2/3 commitments in **82.522 ms** and **83.983 ms**, old-memory reclamation,
reboot into committed monitor 3, reboot into Trusty 2, retained persistent data,
continued secure services, and explicit recovery after committed Trusty image
corruption. Seven refreshed boot-evidence, image/layout, bundle/cache and matrix
gates passed in **91.149 s** (`/tmp/bexos-x86-selected-measurement-gates.log`).
The final host inventory contains **128 targets**, excluding ARM-specific rules
and manual helpers. All **128/128 passed uncached**, total **163.180 s**
(`/tmp/bexos-x86-final-host-matrix.log`, build events
`/tmp/bexos-x86-final-host-matrix.bep.json`, exact inventory
`/tmp/bexos-x86-selected-host-targets.txt`). The first complete 110-target native
invocation was interrupted after **4 passed, 2 failed, 104 skipped**
(`/tmp/bexos-x86-final-native-matrices.log` and
`/tmp/bexos-x86-final-native-matrices.bep.json`). The kernel platform update
failed before replacement in **486.4 s**: its immediate progress query timed out
behind appd's post-launch persistent registry sync. Startup now has a shared
300-second readiness budget; post-cutover checks retain their existing limits.
The normal checkpoint diagnostic failed in **3.7 s** with its fault stack pointer
below the 512 KiB bootstrap stack. The optimized nested aggregate preparation
retains multiple CPU/transport snapshots; this diagnostic alone now receives a
1 MiB bootstrap stack. The corrected checkpoint diagnostic passed uncached in
**371.029 s**, including both-domain restoration, SMP, storage pivot and the
disk-backed application (`/tmp/bexos-x86-matrix-fixes-focused.log`, build events
`/tmp/bexos-x86-matrix-fixes-focused.bep.json`). The kernel rerun passed startup
with persistent progress advancing from 1 to 2, then failed in **664.9 s** when
the existing kernel captured `IA32_PAT` during transplant: the monitor rejected
MSR `0x277`. Total focused invocation time was **1043.508 s**. The monitor now
exposes the architectural guest PAT field, validates all eight memory types,
flushes translations after changes, and validates/preserves PAT during protected
state restoration. All **76 monitor unit tests passed**, target time **0.5 s**,
total **4.605 s** (`/tmp/bexos-x86-pat-host.log`). Standard and acceptance refreshes
then completed sequentially in **6.577 s** and **5.943 s**. Their SHA-256 values
are `ba1ca02ae0e6829217696aa95957507dcdab18112d16a659c7c9db2bb549fe68`
and `59a068cfe8bd423bfb3d09ca2c74ac0893ae7b42cbd5ab8799365dedfdfc03a2`
(`/tmp/bexos-x86-pat-standard-refresh.log`,
`/tmp/bexos-x86-pat-acceptance-refresh.log`). The uncached 128 selected host
targets and native kernel update started against these bundles
(`/tmp/bexos-x86-pat-host-kernel.log`,
`/tmp/bexos-x86-pat-host-kernel.bep.json`). That invocation was interrupted after
**19 passed, 110 skipped** because a WASM host test stalled in Rust's
`stack_overflow::thread_info::set_current_info` before entering its test body.
The read-only process sample is `/tmp/bexos-x86-wasm-grant-hang.sample`.
The same 129 targets reran with `--test_env=RUST_TEST_THREADS=1`, without
skipping assertions: **all 128 host targets passed**. The native kernel executed
its replacement, reported a **3 ms** cutover, resumed debugd and reclaimed old
memory, but its harness failed in **682.0 s** because both status samples already
reported completion. Total invocation time was **743.727 s**
(`/tmp/bexos-x86-pat-host-kernel-serial.log`, build events
`/tmp/bexos-x86-pat-host-kernel-serial.bep.json`). The harness now accepts a fast
completed update without requiring a sample of the transient bulk phase.
Activation and retained requests still enter in one transport write; every
status/health/registry reply must succeed, and trace markers must prove bulk
execution. All subsequent identity, persistent progress, cutover, reclamation
and rollback assertions remain. Full native verification is pending.

The next full native invocation stopped after **4 passed, 2 failed, 104 skipped**
(`/tmp/bexos-x86-native-matrices-restart.log`,
`/tmp/bexos-x86-native-matrices-restart.bep.json`). Its firmware refreshes took
**5.128 s** and **5.915 s**; current bundle hashes are
`8a36bd3e2c47dfe4c3c130fedf5b33f640a392a14832ca09bcf4891ab0cf60ac`
and `3d67953cf56849a43c39900aee23d34cf01b0654782deeb65fbdadd94a743c75`.
Exact artifact manifests are
`/tmp/bexos-x86-native-restart-standard-firmware.prototxt` and
`/tmp/bexos-x86-native-restart-acceptance-firmware.prototxt`. Its source record is
`/tmp/bexos-x86-native-restart-source.json`, SHA-256
`a19a839ffa74cb07b8cc282cbf458e46d96ea359f4b41499935061bae65d806c`.
The Linux submodule matches the earlier checkpoint.

The development network-transplant test passed in **165.8 s**, retaining TCP
across virtio-net, netstackd and keychaind replacements with **25/15/19 ms**
cutovers. Integrated BexFS failed in **248.9 s** because its tracing helper gave
late boot markers only 180 seconds. It now shares the configured 600-second
budget across early readiness, tracing setup and storage pivot. UART failed in
**864.7 s** after its PCI prerequisite passed at **54 ms**: the harness assumed
one running console process, while integrated x86 has debug and RPMB instances.
It now requires exactly one old instance to retire, exactly one new instance to
appear, and every unaffected process (including the peer) to remain unchanged.
Both harness corrections await native verification. The focused BexFS, kernel
and teed prerequisite-chain invocation is `/tmp/bexos-x86-critical-chain.log`
with build events `/tmp/bexos-x86-critical-chain.bep.json`.
The interrupted matrix's owned QEMU and RPMB helper were reaped. The full native
matrix result and final tested-tree record remain outstanding.

ARM Brush is explicitly skipped at the user's request on 2026-09-06. Its last
failure below remains recorded; the skip does not count as a pass. Keep the
scenario in coverage inventories, but exclude
`//testing/e2e/qemu/bexfs:brush_shell_e2e_test_aarch64` from subsequent ARM
acceptance invocations. Work now focuses on the missing secure integration and
live firmware replacement.

## Combined execution checkpoint (2026-09-07)

### X86 recovery implementation checkpoint

The current approved scope is x86 monitor replacement and reboot-based monitor
and Trusty updates. Live Trusty replacement and all ARM tests are excluded from
this effort. **The full approved plan remains incomplete.**
The additions in this checkpoint use HEAD
`f21fc7493e45f6129b57216b39825595186d3b1b` plus the working tree; older
observations below retain their original revision context.

- Distinct-image monitor transfer now executes in an isolated diagnostic. The
  source monitor validates a versioned resume descriptor and identical resident
  recovery instructions, loads a separately linked candidate into a private
  bank, snapshots retained owners, and changes root mappings from a resident
  transition stack. The candidate imports the protected record and overwrites
  and reuses all 320 MiB of the previous image through a private alias.
  `//secure/monitor:monitor_transfer_domain_boot_test` passed in **7.0 s**,
  including real Trusty AVB IPC after old-image reuse
  (`/tmp/bexos-x86-distinct-monitor-transfer.log`).
- `//secure/monitor:dual_monitor_transfer_domain_boot_test` passed in
  **445.1 s**, total **456.357 s**, with the unchanged 600-second boot deadline.
  Both BexOS and Trusty software owners are reconstructed by the different
  monitor image; subsequent evidence includes Trusty IPC, four-CPU startup,
  debugd, WASM, and persistent storage/application progress. This development
  fixture uses the software TEE for BexOS, switches during startup, and does
  **not** establish product activation, retained active product clients,
  the 150-ms cutover budget, durable commitment, or fault rollback. Log:
  `/tmp/bexos-x86-dual-monitor-transfer-frame.log`.
- The initial two-domain transfer faulted and was stopped through its owning
  harness. A bounded resident fault report replaced the uninformative `!`
  marker. Disassembly found that aggregate preparation used an RBP-based
  epilogue without installing its own RBP frame. An aligned scratch address
  materialized before early returns repairs the emitted frame; reconstruction
  also uses the 1 MiB resident stack. A verbose interrupt trace overflowed the
  harness's existing output cap and was a failed diagnostic, not a pass.
- A root-owned ATA PIO firmware-disk backend and a bounded A/B slot-store
  library are implemented. They avoid guest DMA and storage services, complete
  commands synchronously, reject out-of-range requests, and stop using a failed
  controller. Installation authenticates before writing, flushes inactive-slot
  payload and header separately, then rereads and authenticates every byte
  before returning an image identity. The launcher retains `firmware.raw`
  across restarts and refuses to replace an existing disk with invalid geometry.
  Product staging and boot selection are **not yet connected** to this disk;
  the isolated recovery diagnostic described below now selects real images.
- The actual two-boot disk/controller and host-launcher checks passed **2/2**,
  including owner reconstruction and retained disk bytes, total **9.069 s**
  (`/tmp/bexos-x86-firmware-disk-tests.log`). Signed slot-store failure tests and
  C/Rust wire compatibility passed **2/2**, total **4.556 s**. Expanded
  lost-flush-acknowledgement cases passed in the follow-up host run below.
- The x86-only root journal service stores both component commitments, one
  pending trial, bounded attempt count, revision, and the exact last mutation in
  one TP record. Exact retries are idempotent; conflicting component updates and
  stale rollback requests are rejected. Commit updates the legacy component
  journal in the same TP transaction, and legacy mutation cannot bypass the
  new authority once it exists. Migration refuses a legacy generation above
  one rather than fabricating missing image identity information.
- `//secure/monitor:boot_selection_domain_boot_test` passed across **three real
  Trusty boots**, guest **20.9 s**, total **25.795 s**. It checks persisted trial
  state without floor advancement, commit/retry equivalence, atomic legacy
  journal compatibility, stale-abort rejection, and rereading commitment after
  reboot. Its identities are explicit journal fixtures; it does **not** install
  or boot candidate firmware. Log: `/tmp/bexos-x86-boot-selection-guest.log`.
- Candidate ABI/loading and linked resident-layout checks passed **2/2** in
  **8.431 s** (`/tmp/bexos-x86-transfer-host-tests.log`). New diagnostics belong
  to the maintained firmware acceptance suite.
- `//secure/monitor:firmware_recovery_domain_boot_test` passed **62.7 s**,
  total **67.329 s**, in `/tmp/bexos-x86-real-firmware-recovery-interruptions.log`.
  It writes and authenticates an actual separately linked, signed monitor in
  the root disk's inactive slot, publishes pending, restarts QEMU, authenticates
  selection through recovery Trusty, enters the selected monitor, completes
  Trusty IPC, and commits through the protected journal. It deliberately loses
  the commit acknowledgement and resolves it through an authenticated read.
  A later boot executes the committed disk image. Removing that committed slot
  stays in explicit recovery. An independent installation interrupts two trial
  attempts and requires bounded authenticated abort. This is a Multiboot
  diagnostic with no normal-world clients, not the signed EFI product path.
  A first failed run exposed an optimized-away resident ticket write; volatile
  handoff access fixes that boundary. The subsequent four-boot run passed
  **36.5 s** before adding interruption scenarios.
- The shared recovery coordinator authenticates both committed components
  before attempting a pending image, never substitutes an older generation
  for missing committed bytes, and bounds uncertain mutation resolution to
  authenticated reads plus one exact retry. C and Rust final-record validators
  now reject valid request bytes paired with an inconsistent resulting state.
  Uncached `//lib/trusty_boot:recovery_tests`, `//lib/trusty_boot:tests`,
  `//lib/secure_firmware:selection_wire_tests`,
  `//lib/secure_firmware:store_tests`,
  `//secure/orchestrator/trusty:boot_selection_test`, and
  `//secure/monitor:monitor_tests` passed **6/6**, total **9.763 s**
  (`/tmp/bexos-x86-recovery-policy-final.log`). The recovery tests execute the
  actual C authority through a reaped process for each transaction.
- The distinct generation-two Trusty source build and fenced cold-recovery
  diagnostic passed in **55.5 s**, total **62.820 s**, in
  `/tmp/bexos-x86-real-trusty-recovery-fence.log`. The candidate's kernel returns
  its separately compiled generation. Trial RPMB UART writes never reach the
  host; recovery Trusty commits the protected selection, and a fresh boot of
  selected Trusty retains an independent AVB value at location 27.
- Retained AVB client state now crosses the two-image handoff, including its
  pending request and any completed storage reply awaiting delivery. Repeated
  transfer passed with measured readiness **83.854 ms** and **79.891 ms**
  (`/tmp/bexos-x86-retained-monitor-client.log`). The updated dual-image fixture
  passed **348.6 s**, total **360.306 s**, including its 150-ms assertion
  (`/tmp/bexos-x86-dual-transfer-readiness-scheduling.log`). Earlier failed
  readiness runs measured approximately 1.25 seconds; scheduling a normal burst
  during every secure poll caused that delay. Those failures are not passes.
- Permanent NMI, fault, and double-fault IST stacks and entry recovery are
  implemented. Actual candidate-entry invalid-instruction and silent-hang
  tests passed **2/2**, total **21.378 s**
  (`/tmp/bexos-x86-resident-entry-recovery.log`). The old two-image diagnostic
  disarms rollback before VMRUN: its cutover snapshot is not authoritative
  after guest execution advances.

### Permanent execution nucleus checkpoint

A separate nucleus implementation now keeps execution, hardware emulation,
CPU/device/interrupt/timer/DMA owners, transport, and authoritative execution
boundaries in permanent code and memory. The replaceable image contains monitor
scheduling policy, a checked versioned entry descriptor, private data, and a
private stack. It returns bounded work decisions between complete guest
transactions. Resident fault and NMI recovery returns to the nucleus call frame;
it never restores an earlier guest CPU or device snapshot. This is an isolated
native diagnostic, **not yet the production EFI/update path**.

- `//secure/monitor:resident_nucleus_boot_test` and
  `//secure/monitor:monitor_tests` passed **2/2**, total **52.594 s**, native
  **42.2 s** (`/tmp/bexos-x86-permanent-nucleus-first.log`). Signed faulting and
  hanging candidates fail after guest progress, then the retained AVB client
  completes its pending read. Two separately linked healthy candidates commit
  through authenticated reads after deliberately lost acknowledgements before
  old code, writable data, and stacks are poisoned and reused.
- `//secure/monitor:dual_resident_nucleus_boot_test` and
  `//secure/monitor:resident_layout_test` passed **2/2**, total **437.039 s**,
  native **429.9 s** (`/tmp/bexos-x86-dual-permanent-nucleus.log`). Four BexOS
  CPUs, persistent storage, WASM, and application checks continue after the
  same replacement/rollback sequence. The development fixture's BexOS uses
  software TEE; its retained root AVB connection is real Trusty.
- The later nucleus run uses counters published only after actual VMRUN exits
  and completed emulation, plus independent root and normal transport lanes.
  This prevents an unpolled normal reply from blocking journal queries; exact
  late completions remain routed correctly even after revocation and when
  lane ticket numbers coincide. The standalone nucleus passed **17.6 s** in
  `/tmp/bexos-x86-nucleus-and-recovery-serial.log`.
- The same serial run passed the monitor reboot suite in **117.8 s** and Trusty
  reboot suite in **117.5 s**. Both now include deterministic interrupted trial
  cut points, bounded trial exhaustion, missing committed-image rejection,
  retained service data, and power loss after protected commitment but before
  acknowledgement. QEMU and RPMB helpers are reaped after every boot.
- That serial run was **3/4**, not an overall pass: the dual nucleus exceeded
  its unchanged 30-second preparation limit (**51.1 s** failed test). Protected
  store initialization is now separated from live timing, matching required
  boot selection before client admission; preparation gives secure journal
  operations priority while still running BexOS. The isolated rerun passed in
  **346.2 s**, total **356.936 s**
  (`/tmp/bexos-x86-dual-nucleus-preparation.log`). Preparation measured
  **3885.918, 3780.074, 3616.050, and 1993.014 ms**; healthy readiness measured
  **92.136 and 4.644 ms**. This pass predates the independent root-control
  worker and product recovery integration described below.
- An earlier concurrent reboot run passed monitor recovery **186.0 s** but
  failed Trusty recovery **137.9 s** during initial AVB startup
  (`/tmp/bexos-x86-both-reboot-recovery-final.log`). The isolated Trusty rerun
  passed **79.6 s**, total **84.763 s**
  (`/tmp/bexos-x86-trusty-recovery-isolated.log`). Subsequent QEMU validation is
  serialized. Cold boot transport now has an explicit 600-second budget;
  default live transport remains bounded to 30 seconds and readiness to 150 ms.
- One-allocation firmware installation authenticates, writes and flushes, then
  reuses the upload allocation for a complete authenticated reread. Uncached
  `//lib/secure_firmware:tests`, `//lib/secure_firmware:store_tests`,
  `//lib/trusty_boot:recovery_tests`, and `//secure/monitor:monitor_tests` passed
  **4/4**, total **5.814 s** (`/tmp/bexos-x86-resident-storage-host.log`).
  Integration sources were moved out of `tests/` to avoid a Bazel output-prefix
  collision with the existing unit-test executable named `tests`.

The product recovery path now has a separate signed EFI target,
`//boot/efi:authenticated_nucleus_product`. It performs authenticated selection
before admitting normal clients, fences persistent trial I/O, resolves journal
mutations through recovery Trusty, and discards that instance before launching
selected secure services. A separate root-control worker allows protected
journal requests to progress while the normal QL worker handles storage.
Standard and acceptance Trusty source refreshes completed sequentially in
**517.033 s** and **721.137 s**. The ordinary saved EFI product has not yet been
switched to the new nucleus.

The initial signed nucleus build failed because an orphan `.got` section was
linked at address zero. Its linker script now places that table in permanent
data. The product ELF was added to the maintained nucleus layout test;
`//secure/monitor:nucleus_layout_test` and `//boot/efi:embed_monitor_tests`
passed **2/2**, total **12.357 s**
(`/tmp/bexos-x86-product-nucleus-layout.log`). The signed EFI image subsequently
built, and its guest run reached authenticated selection, recovery-instance
disposal, and BexOS services. The full guest result is still pending. A further
correction retains the authenticated committed monitor as the rollback image
for a later trial, rather than assuming it is the embedded baseline; that
correction is not covered by the running guest image.

Remaining product work includes update/activation/status plumbing, complete
trial health and later-generation recovery coverage, final EFI integration,
and the remaining native interruption and negative scenarios. No complete final x86
matrix or final tested tree is claimed. Builds alone do not count as QEMU tests:
all explicit native test commands use `--test_tag_filters=` or the E2E config.

Latest work after the checkpoints below:

- X86 acceptance Trusty refresh completed, total **1678.476 s** including queue
  wait, critical path **546.08 s**
  (`/tmp/bexos-x86-acceptance-journal-refresh.log`). Saved bundle SHA-256
  `35d52b14dbae628b21697ebf662850fe5a30e6606d3966be53923c6ae0b46fea`,
  inner `lk.elf`
  `204f8396243a9ca9cfb630693479b64571d989c55abe296f77f679b5328bb136`.
  Integrated acceptance EFI refresh completed **5.941 s**, SHA-256
  `fef91bc3e324985bb41e01ffff20b0e587e8e71d0ec4238857d27b10e8803d4f`
  (`/tmp/bexos-resident-acceptance-efi-refresh.log`). Its saved Trusty hash
  matches the inner ELF above. Standalone Trusty acceptance and integrated
  AuthMgr acceptance have passed (154 s and 320 s progress markers); the
  monitor-domain acceptance guest remains running in
  `/tmp/bexos-refreshed-x86-acceptance-regressions.log`.
- A further two-domain restoration change separates validation from
  installation for BexOS, Trusty and shared transport. The combined probe
  corrupts the final secure transport record after normal-state preparation,
  requires unchanged snapshots of both old owners, then installs both decoded
  owners and requires later guest progress. It is being built; no combined
  guest pass is recorded yet. This still does not execute replacement code.
- X86-selected repository tests passed **128/128**, uncached, **247.802 s**:
  `bazel --output_base=/tmp/bexos-01a076d8-checks test -c opt --config=x86_64
  //... --build_tests_only --test_tag_filters=-requires-qemu --jobs=1
  --keep_going --nocache_test_results`
  (`/tmp/bexos-x86-repository-tests-checkpoint.log`). This includes the new
  resident-layout check, but excludes QEMU execution and predates the localized
  secure-owner stack-frame repair below.
- Repository-wide matrix coverage passed with `bazel run
  //testing/e2e/qemu:check_matrix` using the secondary output base, **3.644 s**
  for the Bazel run target plus the nested query
  (`/tmp/bexos-resident-matrix-coverage.log`). The launcher now respects
  `BAZEL_REAL` so the nested query uses the selected server. Every runnable QEMU
  scenario belongs to a maintained matrix; this checks membership, not execution.
- Secure-owner restoration now prepares CPU and peripheral objects before
  importing transport state, then installs them only after all nested records
  validate. The QEMU diagnostic corrupts the final transport record, compares
  the entire old CPU/device/transport snapshot after rejection, then discards
  and reconstructs the owner and requires further real Trusty IPC. The initial
  optimized build passed **3.601 s**; the expanded guest regression **failed
  in 60.7 s** with a root fault before restoration evidence
  (`/tmp/bexos-secure-owner-atomic-guest.log`). Exception diagnostics and
  inspection of the emitted restoration function found a missing stack-frame
  setup on a path that uses a page-aligned VMCB local. QEMU confirmed a
  general-protection fault at `FXSAVE`, RIP `0x4016815`,
  with an unaligned stack. A localized repair materializes the aligned scratch
  before early returns; emitted code now has the required frame setup. The
  repaired guest **passed in 32.0 s**, total **39.008 s**
  (`/tmp/bexos-secure-owner-frame-fixed-guest.log`), including unchanged full
  CPU/device/transport snapshots after rejection and continued IPC after owner
  reconstruction with a fetched request in flight.
  Raw fault evidence is `/tmp/bexos-secure-owner-rootfault-qemu.log`.
  This is still same-code state
  restoration, not candidate execution. Rustfmt passed
  `/tmp/bexos-secure-owner-frame-rustfmt.log`.
- Integrated standard EFI refresh completed **6.227 s**, SHA-256
  `157dcb30eff0adae8bfbb16067a6094bc23ef74fd2312854e22391dadcaedf7e`.
  The complete refreshed batch passed **3/3**, total **1285.260 s**:
  rollback rejection **112.6 s**, product rejection **57.7 s**, and
  signed staging **1102.1 s**: explicit tamper rejection,
  authenticated valid images twice, staging reuse and continued Trusty client
  progress. Valid-image activation still returns unavailable; this is not a
  live-replacement pass. Logs are in
  `/tmp/bexos-resident-secure-staging-regressions.log`. The x86 acceptance Trusty
  refresh subsequently completed as recorded above.
- **The bounded scheduling repair passed both original 600-second guest gates**:
  `//secure/monitor:dual_domain_development_boot_test` **432.5 s** and
  `//secure/monitor:normal_checkpoint_domain_boot_test` **352.6 s**, total
  **873.060 s** (`/tmp/bexos-bounded-world-scheduling-guests.log`). The expanded
  reconstruction discards the old normal-domain software owner as well as all
  CPU/register objects, reconstructs retained PCI policy, and requires later
  SMP, WASM, debug, storage/persistence and memory-reuse progress. This still
  executes the same monitor image, not a native monitor replacement.
- Boot approvals now use versioned, bounded records in protected resident
  memory at `0x18074000`; generation/floor/location validation survives retiring
  the software image. Trusty-boot, monitor and linked-image checks passed **3/3**,
  total **6.204 s** (`/tmp/bexos-resident-approval-host-tests.log`). Secure-boot
  guest validation requires the integrated bundle refresh now running. Rustfmt
  passed `/tmp/bexos-resident-approval-rustfmt.log`.
- Refreshed ARM standard regressions passed **2/2**:
  `//testing/e2e/qemu/trusty:trusty_security_stack_e2e_test_aarch64` **133.4 s**,
  `//testing/e2e/qemu/update:kernel_platform_update_e2e_test_aarch64` **174.2 s**,
  with **2 ms** measured cutover, retained client progress and reclamation.
  Total **919.445 s** includes queue wait; log
  `/tmp/bexos-arm-standard-gicv2-regressions.log`.
- The non-checkpoint development baseline also **failed at 601.0 s**
  (`/tmp/bexos-resident-layout-guest-baseline.log`), establishing that the
  startup timeout is not specific to checkpoint restoration. Bounded normal
  bursts (32 exits or 1 ms, yielding immediately for submitted/running Trusty
  IPC) now amortize world switches across fast device exits. Both baseline and
  expanded reconstruction guests are rerunning with the same 600-second boot
  deadline. No guest pass is recorded for this scheduling change yet. Monitor
  and linked-layout checks passed **2/2** in **7.164 s**;
  `/tmp/bexos-owner-resume-scheduling-host-tests.log`.
- The refreshed standard ARM bundle is saved with SHA-256
  `1fb772eb0563f543c6204df77282c1ad41a39307ae845573a1392f4c9ba203a2`.
  Refresh total **1112.523 s** includes queue wait, critical path **526.45 s**
  (`/tmp/bexos-arm-standard-gicv2-refresh.log`). Security-stack and kernel
  replacement regressions are queued against it.
- Real guest checks of the new resident layout passed the SVM DMA probe and
  Trusty CPU/platform/pending-IPC reconstruction (test XML **2 s** and **23 s**).
  The non-checkpoint four-CPU development boot baseline remains running in
  `/tmp/bexos-resident-layout-guest-baseline.log`. An additional protected PCI
  geometry record and owner-independent normal-domain resume constructor are
  implemented. The geometry/ELF layout tests passed **2/2** in **7.440 s**
  (`/tmp/bexos-resident-policy-host-tests.log`), including **65 monitor unit
  cases**; the expanded normal reconstruction probe remains unvalidated.
- The ARM-selected repository checkpoint passed **127/127 tests**, uncached,
  total **1745.875 s**, with `--build_tests_only --test_tag_filters=-requires-qemu
  --jobs=1 --keep_going` (`/tmp/bexos-arm-repository-tests-checkpoint.log`).
  This predates the resident hardware layout below and is not the final gate.
- The monitor now places hardware-referenced VMCBs, NPT/VT-d tables,
  permission maps, root tables and watchdog entries in a fixed protected region
  at `0x18000000`, outside the reclaimable image. Four optimized build targets
  passed **5.497 s**. Linked-image layout, monitor unit and EFI reservation
  tests passed **3/3**, total **5.482 s**
  (`/tmp/bexos-resident-layout-host-tests.log`); actual guest validation is
  queued. This establishes retained hardware placement, not a working recovery
  nucleus or live monitor switch. The mandatory formatter passed
  `/tmp/bexos-resident-layout-rustfmt.log`.
- The four-CPU checkpoint's second 600-second attempt **failed in 601.2 s**
  (`/tmp/bexos-normal-aggregate-clean-retry.log`, total **626.814 s**), reaching
  storage verification startup but not the final application marker. No other
  guest or firmware build from this task ran alongside it, although repository
  host compilation was active. A non-checkpoint boot baseline is queued; the
  earlier contention explanation is not established.
- **ARM AuthMgr now passes with the QEMU GICv2 compatibility patch:** uncached
  `//testing/e2e/qemu/trusty:authmgr_acceptance_e2e_test_aarch64` passed in
  **66.9 s**, including authenticated connection, malformed DICE and
  unauthenticated-completion rejection plus BexOS debug readiness. The same
  invocation passed `//secure/monitor:monitor_tests` in **0.7 s**, including
  identity-linked root-image checks and atomic validation before loading an
  inactive region. Total **93.934 s**; log
  `/tmp/bexos-arm-gicv2-ackctl-validation.log`.
  ARM acceptance bundle SHA-256:
  `bc3654dbb2a2076349e867c461ac50716d303e980b9c78acf15d75af8a71c1c4`.
  Refresh total **2082.696 s** includes queue wait; critical path **783.77 s**.
  The ARM standard bundle still needs the same explicit refresh and regression
  coverage. The required formatter passed
  `/tmp/bexos-staging-evidence-rustfmt.log`.
- **Protected journal guest validation now passes.** The uncached
  `//boot/efi:authenticated_rollback_rejection_test` completed all five cases
  (test XML: **115 s**, zero failures): payload/Trusty/monitor AVB floors and
  Trusty/monitor journal commitments. Journal fixtures exercised persistent
  reopened reads, identical retries, conflicting commits and explicit product
  reboot rejection. The companion firmware-staging guest failed in **1242.4 s**
  at its evidence assertion despite the valid candidate's authentication marker
  appearing in the guest console. The harness retained an offset into a bounded
  trace ring across a long upload and inspected independent console diagnostics
  before draining them after the RPC response. The corrected evidence window
  needs a guest retry. This is not a completed replacement or full-suite result. Log:
  `/tmp/bexos-journal-cold-boot-tests.log`.
  Standard Trusty refresh completed **480.818 s**, bundle SHA-256
  `87d61144b5a67620fa56796a86848085b13009aa6fb33297cc96b75fedb08a85`;
  integrated refresh completed **5.408 s**, bundle SHA-256
  `210406216a2162fc01b5fd05a61a6e732de8644da1606824897e425287c5212d`.
  Both contain Trusty ELF
  `4790f05da33cec8d48c0f4bd56feccb4ad9e9e06341129b55af1e37b5a71eb7f`.
- Four-CPU normal-domain aggregate records are implemented with complete
  pre-install validation, retained NPT/device policy and no imported host
  pointers. Product and diagnostic builds passed **6.312 s**. The new
  `//secure/monitor:normal_checkpoint_domain_boot_test` corrupts the fourth CPU
  record to check atomic rejection, discards all four CPU/register objects,
  restores the protected record and requires subsequent BexOS boot/application
  progress. It is included in the maintained firmware acceptance matrix. Its
  first run timed out in **601.0 s** after the restore marker, SMP, WASM and
  storage verification, while another guest was running. A clean retry with
  the unchanged 600-second deadline is required. This is another reconstruction gate, not activation
  of a different monitor image.
- Repository tests selected for x86 completed with **126 passing and one
  failing to build**, rather than a full pass. The failure was validated saved
  firmware input mismatch (`/tmp/bexos-journal-x86-repository-tests-only.log`).
  The maintained E2E inventory filegroups and uncovered-tests genquery now
  carry `testonly = True`, fixing their dependency analysis under `//...`.
- A queued `bazel run` does not serialize its post-build refresh launcher with
  the next build. The first integrated refresh captured the previous Trusty
  ELF. Waiting for the entire refresh process before rebuilding corrected the
  mismatch: Trusty bundle `8c04d8069c4804d7e9261f5b3dde49237c01bf227ce16c270e26ed1a0a10ba6a`
  and integrated bundle `3b92907b80f3897388fa9185e4c9a03a307359fbf63ea82c4f9ba86c94d05174`
  both contain ELF SHA-256
  `5780e4ae0114bab3243119a0f675bebf4c7ad4452f04a3bf30bb322563703436`.
  These are diagnostic checkpoints, not accepted final firmware.
- The refreshed x86 rollback and staging guests **failed** in **64.1 s** and
  **10.1 s** (`/tmp/bexos-journal-staging-ordered-tests.log`). Product boot
  refused entry because its new protected journal was unavailable. A focused
  diagnostic guest (**7.3 s**, failed) returned kernel IPC `ERR_TIMED_OUT`
  while TP was discovering RPMB geometry. The journal adapter now shares a
  single 30-second budget across connect and response waits instead of a
  one-second limit per wait. The refresh and five-case journal guest pass above
  validate this change; it does not extend the 150 ms cutover limit.
- ARM deferred IRQ forwarding passed kernel replacement with **3 ms** cutover,
  but security stack and AuthMgr both failed with an interrupted standard call
  exhausting its retry bound, followed by Busy for later calls. Diagnostic
  run `/tmp/bexos-arm-irq-source-diagnostic.log` (**96.5 s**, failed) identified
  pending interrupt **29** during repeated restarts. The QEMU GICv2 model lacks
  AIAR/AEOIR/AHPPIR, used by pinned Trusty's IRQ handler. An ARM QEMU-only patch
  selects the implemented main interface with secure AckCtl, preserving Group
  0 FIQ routing. Firmware compilation and guest validation are pending. Source:
  [QEMU GICv2 implementation](https://github.com/qemu/qemu/blob/master/hw/intc/arm_gic.c).
- The real Trusty checkpoint guest passed again in **11.3 s** against the
  journal-enabled Trusty image. This remains object/state reconstruction in
  the same monitor code, not monitor-image replacement.

- The x86 regression batch finished **6/7 passing** in **4472.682 s**
  (`/tmp/bexos-x86-final-staging-secure-regressions.log`). Security stack passed
  **1265.6 s**, integrated AuthMgr **414.3 s**, debugd state replacement
  **727.6 s**, standalone Trusty acceptance **169.3 s**, cache boundary
  **0.3 s**, and cached replacement authentication **0.4 s**. Debugd retained
  its original host connection and partial upload; measured cutover **83 ms**.
  Firmware staging failed **1807.1 s**: the valid candidate received
  `ErrAccessDenied`. Its Begin opcode collided with evidence confirmation
  (`0x200`). Source now reserves `0x210..0x213` for firmware transfer and asserts
  disjointness. A refreshed monitor and focused guest retry are required.
- The host update client now sends bounded 60 KiB chunks under the existing
  64 KiB frame limit, instead of thousands of 4 KiB round trips per firmware
  image. Byte reconstruction, fragmented acknowledgements and immediate stop
  on rejected chunks passed with both debug-client targets (**4.470 s**,
  `/tmp/bexos-firmware-upload-chunk-tests.log`). Preparation and cutover limits
  remain 30 seconds and 150 milliseconds.
- The actual saved Trusty guest passed CPU, device and root-transport object
  reconstruction, including a request stopped after secure Fetch, in **5.9 s**
  (`//secure/monitor:trusty_checkpoint_domain_boot_test`,
  `/tmp/bexos-trusty-platform-checkpoint-guest.log`). It required continued AVB
  progress and the pending-request marker after discarding the old objects.
  This tests retained execution state, not entry into different monitor code.
  Root records include pinned registrations, pending mailbox/ticket, completion
  buffer and boot evidence. Validation completes before installation.
- The 62 monitor host tests cover pending interrupt delivery, original timer
  deadlines, incomplete UART output, RTC SET state, PCI BAR probes and virtio
  negotiation. Monitor/ABI checks passed **4.802 s** after transport validation
  changes (`/tmp/bexos-transport-handoff-host.log`); runtime/probe build passed
  **4.030 s** after fixing a moved validation closure. Updated SVM CPU/fabric
  probes passed **0.3 s** and **1.6 s** in the ARM-selected batch.
- The first ARM interrupt-ownership retry failed all three product guests at
  early boot because its new diagnostic allocated before heap initialization.
  Replacing that log with a static message allowed security stack (**115.1 s**)
  and kernel replacement (**135.3 s**, **2 ms** cutover and reclamation) to pass.
  AuthMgr's provider then failed QL creation with repeated secure-call Busy;
  that failed guest was explicitly stopped and reaped at **294.2 s**, not
  counted as an acceptance pass. Log: `/tmp/bexos-arm-irq-allocation-fix.log`.
  Source now forwards each CPU's Trusty timer/IPI wakeups through concurrent NOP
  after EOI, rather than relying only on CPU0 provider polling. Its kernel build
  passed **6.549 s**; an instrumented AuthMgr guest retry is queued.
- A kernel-UUID-only Trusty orchestrator journal service is now implemented in
  source. Its TP storage transaction commits slot, image hash, previous/current
  generation together, validates architecture/component, supports exact retry
  after a lost acknowledgement, and rejects conflicting/stale commits. Root
  boot approval checks these records plus AVB component floors before sealing
  and entering BexOS. Normal QL clients retain zero UUID and cannot submit the
  reserved root operation. No live activation caller commits this journal yet.
  Host journal/client/ABI targets passed **3.784 s** and product/fixture monitor
  builds **4.756 s**. A signed fixture now exercises commit, reopened query,
  idempotence, conflict rejection and product rejection after reboot; guest
  validation needs refreshed firmware and is pending.
- Monitor discovery's boot-services bit now has matching acceptance, worker
  and kernel handling. This requires refreshing both standard and acceptance
  Trusty caches before regenerating integrated EFI bundles. The standard x86
  refresh is running. The mandatory formatter passed
  `/tmp/bexos-journal-transport-rustfmt.log`; later IRQ/journal-fixture edits
  require another formatter run. Current saved hashes below predate this work.

- The ARM pre-entry/security and SVM regression batch completed with **3/4
  passing**, Bazel **726.907 s** (`/tmp/bexos-arm-preentry-and-cpu-guest-regressions.log`).
  `//testing/e2e/qemu/trusty:trusty_security_stack_e2e_test_aarch64` passed in
  **115.6 s**, including real service operation/reboot persistence, a separately
  authenticated BL33 provisioning fixture, explicit stale RPMB generation
  rejection before kernel entry, and corrupted RPMB authentication rejection.
  `//secure/monitor:svm_probe_test` passed in **0.3 s** and
  `//secure/monitor:svm_dma_probe_test` in **1.2 s**. These probes resumed actual
  interleaved guest execution after CPU-record restoration and delivered a
  previously masked virtual interrupt after restore.
  `//testing/e2e/qemu/trusty:authmgr_acceptance_e2e_test_aarch64` **failed in
  600.8 s**: its readiness waiter slept once and did not wake after KeyMint
  initialization. Legacy Trusty's GICv2 timer uses a normal-world interrupt
  masked by kernel startup. A scoped secure-call interrupt ownership change,
  including kernel-transplant preservation, is written but not yet validated.
- Saved ARM bundle format 2 now includes the separately signed rollback
  fixture. Standard refresh passed in **505.235 s**, SHA-256
  `b0a59f26fd8a908494bd186b5729d12d80d305710a4a64a09989d8463f5b6446`;
  acceptance refresh passed in **510.524 s**, SHA-256
  `ad32d358382ca01128e1416b103f3581ce45b48322ddf0b01b452bd118ee500b`.
  Logs: `/tmp/bexos-arm-authenticated-fixture-refresh.log` and
  `/tmp/bexos-arm-acceptance-preentry-refresh.log`.
- Four host targets (monitor, QEMU harness, BL33 heap, Trusty bundle) passed
  uncached in **8.791 s** (`/tmp/bexos-cpu-state-host-tests.log`), and the required
  formatter completed. Integrated x86 acceptance was refreshed again in
  **4.593 s**, SHA-256
  `17e24ea3646a7b8bf937dafb4a369b4c944642eb33f10ec864cc3cc89cac0e37`.
  The seven-target x86 staging/security/acceptance/update regression retry is
  running in `/tmp/bexos-x86-final-staging-secure-regressions.log`; its results
  are pending. Subsequent interrupt-fabric checkpoint source and probe changes
  are also pending validation. None of these checkpoints implements actual
  Trusty activation or live monitor replacement.

- The corrected pre-entry ARM smoke test passed in **31.6 s**, Bazel
  **36.990 s**, using saved firmware SHA-256
  `da680a8fa2508364978e8758cf2a55dbf99291a9553aff3caa269a22110d8a5c`.
  Its log contains BL33's authenticated Trusty rollback approval before kernel
  entry and the kernel's RPMB evidence verification, followed by the normal
  storage smoke lifecycle. `/tmp/bexos-arm-preentry-irq-smoke.log` records this
  checkpoint. It predates the allocator correction and signed rollback fixture.
  The next explicit ARM standard/acceptance refreshes are running with those
  changes; their final guest results remain pending.
- Both x86 acceptance clients' readiness refresh passed in **547.540 s**,
  followed by the integrated acceptance refresh in **8.364 s**. Saved hashes:
  Trusty `55589ea6cbe431669f1e73d7837cf8bc26c8f1f0bacb7526cc5d3a54bfec1a5e`,
  integrated `790201577938e1eae4eb2c3202928e8550a6aeccd300affdeeb978b4021710c2`.
  Actual acceptance guest retries remain pending.
- Protected mailbox/registration records preserve pending requests and replies,
  monotonically allocated identifiers, ownership and exact mapping bounds.
  Restoration recomputes host addresses from resident RAM policy. The BL33
  allocator now aligns actual returned addresses and checks padding/overflow.
  Five focused test targets passed uncached in **3.637 s**
  (`/tmp/bexos-arm-fixture-host-tests.log`), after an earlier four-target pass
  in **4.280 s**. CPU record source and a real interleaved-guest restore probe
  have since been added and still require validation. These records are not a
  complete monitor replacement backend; actual activation remains unavailable.

- The first refreshed ARM pre-entry smoke run failed in **32.1 s** (Bazel
  **96.295 s**) before kernel entry. A pending normal-world interrupt repeatedly
  interrupted Trusty bootstrap; BL33 explicitly refused execution when its
  bounded call expired. BL33 now masks only the non-secure GICv2 interrupt group
  while it drives Trusty bootstrap and approval, restoring it before kernel
  entry. The corrected firmware refresh and guest retry are pending.
  `/tmp/bexos-arm-preentry-smoke.log` records the failure. The superseded firmware
  refresh completed in **728.234 s**, SHA-256
  `7758edd473dd8b122773c06f7e42adf9fe312d9536603f8405961033b0be4be4`.
- The broad x86-selected repository run passed **110/111** targets in
  **514.867 s**. Its remaining debug-client failure was a macOS socket deadline
  setup race after peer shutdown with unread response bytes. The transport now
  drains that socket nonblocking under the existing deadline. Both debug-client
  test targets then passed **20 repetitions each** in **6.428 s**; the broad
  final retry remains pending. Logs: `/tmp/bexos-repository-x86-preentry-rpc.log`
  and `/tmp/bexos-debug-socket-drain-validation.log`. The required repository
  formatter and the Bazel-managed acceptance-overlay formatter completed.

- The staging/security regression batch finished with **3/7 passing** in
  **2495.964 s** (`/tmp/bexos-staging-secure-guest-regressions.log`). Standalone
  Trusty acceptance passed in **169.7 s**, cache-boundary in **0.4 s** and
  replacement authentication in **1.3 s**. Four failures remain recorded:
  security-stack completed enrollment and reboot but its corrupted-RPMB check
  stopped at the storage TA's deliberate critical exit before observing the
  monitor's refusal; debugd-state reached the app update and hit a nested RPC
  timeout; AuthMgr's separate acceptance client wrote storage during boot-owner
  release. The harness now permits only the exact storage critical-exit line
  after bad-MAC evidence, still requires the monitor's explicit rejection, and
  still rejects unrelated panics. Update RPCs preserve late-reply ownership in
  debugd state version 11 and updated runtime state version 2. Both acceptance
  clients now share the verified-KeyMint readiness gate. Guest reruns and the
  new acceptance firmware refresh are pending; these are not passing results.
  Security-stack failed in **1257.3 s**, debugd-state in **511.9 s**, and AuthMgr
  in **23.5 s**. Firmware staging failed in **428.6 s** before transfer because
  its bundle argument used an execution path instead of a runfiles path; the
  Bazel argument is corrected. No staging guest authentication pass is claimed.
- ARM BL33 now has source code for pre-kernel Trusty AVB approval with a
  heap-free QL storage pump and a bounded, polled QEMU PCI UART transport.
  The same RPMB helper accepts the normal-world connection after a frame-boundary
  release. The verifier compiled with `--config=aarch64` in **2.129 s** after
  correcting an inline-assembly register declaration. No refreshed ARM image or
  guest result is claimed yet. Physical-board transport and coherency remain
  outside this QEMU implementation.
- Protected replacement lifecycle records now bind architecture/component,
  owner slots, generations, original preparation/cutover timestamps and
  uncertain-commit state in a canonical 160-byte versioned record. Its checksum
  detects corruption; protected-memory provenance is required separately.
  This is not a complete monitor CPU/device handoff or live activation backend.
  Monitor, orchestrator and slot tests passed uncached in **5.440 s**
  (`/tmp/bexos-replacement-state-wire-tests.log`). Six focused boot-storage,
  QEMU, update-service, debug-client and slot test targets passed in **10.799 s**
  (`/tmp/bexos-boot-rpc-host-tests.log`); debugd and updated guest binaries built
  in **8.732 s**. The required formatter completed, and the process audit found
  no remaining QEMU guests or RPMB helpers from the completed batch. The explicit
  ARM firmware refresh and broad x86-selected repository tests are running.

- The acceptance readiness refresh completed in **519.312 s** (516.80 s
  critical path), and its integrated cache refresh completed in **5.953 s**.
  The optimized `//device/virtual/qemu/nongui:run` and `:virtual_x86_64` product targets
  and affected E2E executables built successfully in **59.230 s**. The required
  formatter completed. Uncached staging, security-stack, AuthMgr, debugd-state
  and standalone acceptance regressions are now running in
  `/tmp/bexos-staging-secure-guest-regressions.log`; no guest pass is claimed yet.

- The cached service/RPC batch in `/tmp/bexos-secure-rpc-acceptance.log`
  finished with **4/7 passing** in **1976.837 s**. Standalone Trusty acceptance
  passed in **169.2 s**, as did bundle, cache-boundary and archive checks. The
  security-stack guest completed enrollment, wrong-password rejection,
  password replacement and old-password rejection, then failed its second
  launch in **711.2 s** because copying onto a read-only staged EFI loader was
  denied. The launcher now copies and renames a complete temporary file;
  its new repeated-launch/failed-copy regression passes. Reboot persistence
  still needs a guest retry. Debugd state-update failed in **427.1 s** because
  unavailable monitor activation was mislabeled as `ErrPeerClosed`; an explicit
  `ErrUnavailable` status now crosses the TEE interface. Integrated AuthMgr
  acceptance failed in **23.9 s** when its standalone security client wrote
  storage during RPMB handoff. The acceptance client now waits for the verified
  KeyMint root and shared secret in integrated mode, then runs its real service
  checks without overriding boot information. Its firmware refresh is pending.
- Authenticated candidate staging is now implemented in the product monitor:
  registered 64 KiB chunks are copied into root-owned memory and sealed after
  independent AVB signature, exact-image, architecture, component and generation
  checks. Abort, pin revocation and a fixed 30-second preparation deadline erase
  the snapshot. The provider supports full-sized candidates; update/TUF/FIDL
  interfaces have a distinct hypervisor component. **Activation remains
  unavailable**, and these checks do not establish live replacement. The
  affected monitor/provider/kernel/service binaries compiled in **17.611 s**.
  Ten focused host targets passed in `/tmp/bexos-staging-regressions.log`; the
  new TUF test initially selected different host/production library variants,
  was corrected, and passed separately in **0.5 s**.
- Standard integrated cache format 2 was refreshed explicitly through Bazel
  in **4.553 s**. It includes signed current-code Trusty/hypervisor staging
  artifacts at generation 2. SHA-256:
  `dad75e29c60206e0087c56b0ed362b95e511c42041b0a316c8bb8ecf271e23f7`.
  `//boot/efi:cached_replacement_authentication_test`, bundle validation,
  cache dependency boundary and matrix coverage passed uncached in **3.559 s**
  (`/tmp/bexos-replacement-cache-validation.log`). The maintained x86 matrix
  now includes `//testing/e2e/qemu/update:firmware_staging_e2e_test_x86_64`;
  its guest run is pending. It exercises full-sized transfer, tamper rejection,
  abort/reuse and retained clients, and explicitly refuses to count staging as
  completed replacement.

- `//boot/efi:authenticated_boot_approval_test` passed uncached in 39.1 s,
  including two authenticated approvals using the same RPMB state and explicit
  firmware/isolation rejection cases.
- The secure-product guest confirmed version-3 boot evidence against the
  resident monitor and exercised the real Trusty provider, normal-world RPMB
  console, KeyMint verified boot information and authentication-token key.
  This required architecture-specific kernel bootstrap and a fix for repeated
  ELF driver launches: the resolver retains ownership of borrowed executable
  VMOs, while each instance receives private writable segments. All 112 appd
  host tests passed.
- The first complete-product attempt then timed out in its page-reuse proof.
  Untouched anonymous VMOs are lazy and cannot prove physical page allocation.
  The proof now uses physically backed, bounded probes. The uncached retry
  `//boot/efi:authenticated_secure_product_test` passed in **765.4 s**
  (778.504 s invocation), including two full boots with the same disk and RPMB,
  persistence across reboot, WASI, four BexOS CPUs, real Trusty services and
  explicit modified monitor/Trusty, disabled Secure Boot, unprotected variables
  and unavailable-isolation rejections. Log:
  `/tmp/bexos-secure-product-physical-reuse.log`. This checkpoint predates the
  external-payload/harness changes below.
- An external-payload monitor variant now snapshots kernel, BootFS and vbmeta
  into protected memory using bounded fw_cfg DMA, then verifies AVB with its
  embedded root. Missing files, size mismatches, modified content, signed wrong
  policy and zero generation have explicit guest rejection scenarios. The
  maintained x86 launcher is being connected to this EFI/SVM path, preserving
  one RPMB helper and transferring its connection only after boot-owner release.
  `//boot/efi:authenticated_external_secure_product_test` subsequently passed
  uncached in 757 s, including both full boots, those 12 external-payload
  rejection cases and the firmware/isolation rejections. The tested signed EFI
  image SHA-256 is `9360ef31f3c30b20457d13b06746d655beb93b27014706f6b23ab7257fa634d1`.
  QEMU harness tests (10), monitor tests (47) and matrix coverage also passed.
  The first maintained native x86 NVMe E2E was canceled after QEMU exited:
  the relay's retained socket clone prevented EOF and deadlocked cleanup.
  It has no passing result. Explicit write-side shutdown fixes the relay;
  the follow-up QEMU host suite passed all 12 tests, including EOF before
  owner drop, backpressured cancellation and child reaping. The maintained
  `//testing/e2e/qemu/nvme:nvme_smoke_test_x86_64` retry passed in **391.0 s**,
  including the integrated secure guest and completed process cleanup. Original log:
  `/tmp/bexos-external-secure-e2e.log`.
- Later changes add consistent standard/acceptance bundle selection across the
  monitor, signed policy measurement and RPMB artifacts; the x86 product now
  declares `secure_world: TRUSTY`, and its `run` target selects authenticated
  EFI/SVM. A separate signed provisioning fixture prepares an actual higher
  Trusty RPMB floor, then boots the ordinary product to require stale-generation
  rejection before kernel entry. Acceptance-variant service operation still
  requires its refreshed saved firmware and guest validation.
  The current follow-up log is `/tmp/bexos-native-secure-retry.log`. Its first
  x86 BootFS architecture check rejected the portable `.wasm` boot fixture
  because it assumed every `bin/` entry was ELF. The checker now validates
  WASM core/component headers separately. The rollback fixture provisioned its
  counter successfully, but its second QEMU launch exceeded macOS's Unix-socket
  path limit. That setup error did not count as rejection evidence. Both corrected
  scenarios passed uncached in `/tmp/bexos-rollback-corrections.log`:
  `//boot/efi:authenticated_rollback_rejection_test` in **30.4 s**,
  `//testing/build/architecture:x86_64_bootfs_test` in **0.7 s**, and
  `//tools/qemu:qemu_tests` (12 tests) in **0.4 s**. The rollback guest explicitly
  reported `stale generation rejected by Trusty RPMB floor` before kernel entry.
- The clean ARM console transplant rerun
  `//testing/e2e/qemu/update:rpmb_console_update_e2e_test_aarch64` passed uncached
  in **207.6 s** (258.246 s invocation), including rejection, incompatible state,
  candidate failure, preparation timeout and successful replacement with
  retained RPMB clients. Log: `/tmp/bexos-arm-console-clean.log`. No competing
  firmware build ran during this guest; the earlier enrollment failure did not
  recur. This replaces that failed checkpoint, not the final ARM matrix gate.
- Kernel loading now rejects all PT_LOAD memory, including zero-filled tails,
  that overlaps the handoff page, BootFS, evidence or replacement workspace.
  `//boot/efi:authenticated_product_rejection_test` passed uncached in **51.4 s**:
  unsigned/wrong-key/revoked product loaders, 12 external-payload failures, and
  five correctly signed invalid kernels (architecture plus four reservations).
  Monitor host tests, actual-image AVB verification and matrix coverage passed
  in the same invocation. The security-stack test failed to compile because of
  temporary marker-slice lifetimes; that harness error was corrected and its
  separate guest retry is pending. Log: `/tmp/bexos-security-layout-stack.log`.
  The required Bazel Rust formatter completed after these changes.
- The next real x86 security-stack run reached and passed KeyMint algorithm and
  secure-deletion checks, then failed in **407.6 s** because the concurrent AVB
  probe required a nonzero rollback floor. Boot approval deliberately does not
  advance that floor. The probe now compares the actual floor before and after
  queued AVB/orchestrator requests. This was a probe expectation failure, not a
  rejected write request. Log: `/tmp/bexos-x86-secure-services.log`.
- The external monitor now reserves bounded 64 MiB kernel, 128 MiB BootFS and
  64 KiB metadata snapshots independent of current image lengths. AVB must
  authenticate their exact lengths, including rejecting trailing metadata.
  The monitor constructs the four-CPU x86 handoff itself and derives its public
  verification root independently of guest image generation. Product launchers
  consume `//boot/efi:cached_firmware`, validated against the selected saved
  Trusty, pinned OVMF code and AVB root. The cache includes the signed loader,
  monitor, Trusty, RPMB helper/template, enrolled/revoked variables and a signed
  rollback fixture. Explicit refresh is
  `bazel run -c opt --config=x86_64 //boot/efi:refresh_firmware`; acceptance also
  selects `--//build/platforms:trusty_variant=acceptance`. Generated bundles are
  ignored by Git. Guest and cache-boundary acceptance remains in progress in
  `/tmp/bexos-cached-secure-acceptance.log`. The acceptance Trusty bundle still
  needs its boot-proxy release refresh. Firmware bring-up probes remain explicit
  source-built diagnostics outside the ordinary product matrix.
- The first cached-firmware batch passed six of eight targets: product
  rejection **55.8 s**, persistent rollback rejection **26.6 s**, monitor and
  exact-image verifier **0.5 s each**, matrix coverage **0.3 s**, and AVB tooling
  **0.7 s**. The cache archive unit fixture failed because Python 3.14 forbids
  constructing a nonempty tar member without a source stream; its oversized
  input test now lowers the reader's bound against a real member instead.
  The service guest passed KeyMint, orchestrator, AuthMgr availability and
  concurrent calls, then failed in **463.3 s** when debugd's 60-second outer
  user RPC expired during encrypted-volume provisioning. The nested filesystem
  already permits 300 seconds. User mutation RPCs now allow 360 seconds and
  host requests 600 seconds; VFS disk-image and filesystem-control calls use
  the existing 300-second durability budget. Debugd fences late untagged user
  replies, preserving that state in migration record version 10. Authentication
  lifetime and secure-replacement deadlines are unchanged. These RPC changes
  await guest regression; no user-enrollment success is claimed yet.
- Seven follow-up host targets passed uncached in **11.736 s**: archive and
  integrated-bundle validation, the entire x86 product/matrix firmware dependency
  boundary, debug client/framing, debugd and VFS. The modified debugd and VFS
  guest binaries compiled under x86 in **7.652 s**. The required formatter
  completed afterward. The acceptance Trusty refresh completed with a
  **478.79 s** critical path (**759.264 s** including its wait for the active
  test invocation). Both integrated variants were then explicitly refreshed:
  standard **1.938 s**, acceptance **4.569 s**. Their service, AuthMgr and debugd
  transplant regressions are running in `/tmp/bexos-secure-rpc-acceptance.log`.

The narratives below retain earlier checkpoints and failures; the newer dated
results above supersede their implementation-status descriptions.

The fw_cfg DMA descriptor and port ordering follow the
[QEMU firmware configuration specification](https://www.qemu.org/docs/master/specs/fw_cfg.html).
The snapshot transport is not an authentication mechanism; AVB verification
must succeed before normal-world entry.

The following uncached optimized x86 targets passed with `--test_tag_filters=`:

| Target | Result / guest test time |
| --- | --- |
| `//secure/monitor:dual_domain_development_boot_test` | passed, 258.7 s |
| `//secure/monitor:dual_domain_acceptance_boot_test` | passed, 538.3 s, two boots |
| `//boot/efi:authenticated_dual_domain_development_test` | passed, 263.2 s |
| `//boot/efi:authenticated_dual_domain_acceptance_test` | passed, 518.6 s, two boots |
| `//secure/monitor:svm_dma_probe_test` | passed, 0.8 s in the combined checkpoint |

These execute BexOS on four virtual CPUs alongside real Trusty inside one SVM
machine. Required BexOS evidence includes CPU3 scheduling, HPET/IOAPIC interrupt
delivery, ring-3 appd, the debugd virtio socket, WASI randomness, BexFS mounting,
disk-only application execution and persistence. Trusty acceptance checks its
services and advances persistent storage through both boots. BexOS still uses
the software TEE provider in these fixtures; the two domains' startup success
does not prove their service transport or the product boot-approval chain.

VT-d translates assigned NVMe and modern virtio device DMA into BexOS RAM.
The physical DMA probe requires actual IOMMU fault records and unchanged
sentinels for writes targeting monitor and Trusty memory. Physical MSI entries
are all invalid and INTx is disabled; guest interrupt controllers are virtual.
Virtio ACCESS_PLATFORM negotiation is enforced before queues or device-specific
writes are enabled. The network and console drivers negotiate that feature.

The first combined EFI attempts failed before guest entry because the loader's
own large signed payload overlapped the fixed BexOS RAM reservation. The loader
now reports the conflicting memory descriptors, and the BexOS host RAM bank is
`0x30000000..0x60000000`, disjoint from Trusty's `0x20000000..0x30000000` bank.
The passing EFI fixtures use 3072 MiB. The EFI harness now waits for every
required marker and rejects guest failures even after an early Trusty marker.
Embedded monitor/Trusty modifications, disabled Secure Boot, unprotected
variables and unavailable SVM produce explicit rejection in these tests.

The later transport work adds kernel-owned pin validation, revocation on unpin
and process retirement, a bounded monitor mailbox, an x86 QL provider adapter,
and an in-tree Trusty adapter using upstream QL authorization with private
bounce buffers. The kernel/ABI/monitor host tests passed (122 kernel core tests)
and the monitor/provider compiled. `bazel run -c opt --config=x86_64
//third_party/trusty:refresh_x86_64_image` rebuilt the standard saved bundle in
468.816 s. The new `//secure/monitor:trusty_boot_ipc_boot_test` passed in 5.7 s,
requiring a decoded reply from Trusty's real AVB rollback-counter endpoint.
Standalone `//third_party/trusty:x86_64_boot_test` passed in 1.3 s, and matrix
coverage passed in the same uncached 11.222 s invocation. This boot-owner client
has not yet been used to approve BexOS entry, and normal-world service acceptance
remains pending. The passing combined EFI checkpoint predates these transport
edits and must be repeated after integration.

The acceptance bundle was explicitly refreshed through
`//third_party/trusty:refresh_x86_64_acceptance_image` in 502.012 s. Its standalone
`//third_party/trusty:x86_64_acceptance_test` passed uncached in 165.7 s. A later
`//secure/monitor:trusty_boot_approval_boot_test` passed in 8.9 s: the root monitor
verified the actual signed kernel/BootFS/policy/vbmeta fixture, checked Trusty's
locked state and RPMB rollback floor for generation 2, sealed boot mutations,
and required an explicit Trusty rejection of a subsequent rollback write. This
diagnostic does not enter BexOS and does not establish secure-product acceptance.
The authenticated EFI version and proxy ownership release are being integrated.

The verifier's heap dependency was removed by selecting the already pinned,
allocation-free SHA-256 0.11 dependency. The combined direct boot then exposed
that its Multiboot page tables occupied the new BexOS RAM bank. The monitor now
installs private root page tables before clearing guest memory. The serial
driver now selects exact `debug0`/`rpmb0` device names, and appd consumes the
driver readiness acknowledgement before querying that role. The latest combined
development regression reached storage verification but exceeded its 300 s
deadline; it is a failure, not a replacement for the historical passes above.

Eight ARM-selected host targets passed after the shared approval and serial
changes: kernel core, AVB, monitor ABI, boot client, monitor, Trusty provider,
appd and virtio console. A subsequent four-target x86-selected checkpoint passed
boot-client fatal-error handling, boot-owner operation/identity boundaries,
monitor tests and matrix coverage. The full-repository QEMU inventory now includes
monitor/EFI/standalone firmware and Multiboot acceptance suites.

The ARM `//testing/e2e/qemu/update:rpmb_teed_update_e2e_test_aarch64` regression
passed in 265.6 s. The console counterpart failed in 229.1 s during user
enrollment after its four rollback checks; its successful replacement case was
not reached. An overlapping secondary x86 build was canceled, and the console
case still requires a clean rerun. The second explicit standard-firmware refresh
completed with a 458.81 s critical path (928.727 s including the Bazel server
wait). It adds bounded boot RPMB proxy release after complete frame exchanges
and IPC closure. The acceptance bundle has not yet received this later change.

The integrated secure-product path is now implemented but unvalidated. It
requires the versioned authenticated EFI entry contract, verifies payloads and
obtains Trusty approval before normal-world entry, releases the boot proxy,
assigns a separate normal-world RPMB virtio function, and publishes version-3
evidence that the kernel confirms against the monitor's private digest. The
signed x86 policy measures the actual saved Trusty ELF. QMP moves the single
RPMB helper connection from the boot UART to the normal-world virtio function.
`//boot/efi:authenticated_secure_product_test` requires two complete boots and
all BexOS, Trusty approval, transport and persistence markers. It is not a pass
until those requirements succeed. The combined TCG bring-up budget is now 600 s
per boot; secure replacement's 30 s preparation and 150 ms cutover requirements
remain unchanged. `bazel run @rules_rust//:rustfmt` completed after this wiring.

The first integrated checkpoint passed six shared host targets, explicit secure
monitor refusal of direct Multiboot entry (0.8 s), and real Trusty boot RPMB
owner release (8.2 s). EFI approval failed its RAM reservation, and the product
EFI container exceeded the payload bound because it embedded two BootFS copies.
The payload verifier and loader now share the same embedded bytes; the larger
approval fixture uses 3072 MiB. These EFI corrections await guest validation.

## Implemented boundaries

`//boot/efi:secure_boot_test` builds a signed EFI diagnostic through Bazel and
boots it under pinned QEMU OVMF with SMM. It checks SecureBoot/SetupMode, rejects
an unsigned platform-key deletion, and attempts a real flash byte-program write
outside SMM. An unprotected-flash negative control proves that the write test
detects missing protection. The firmware code drive is read-only; each case
gets a private variable store and EFI disk. Public development credentials are
test material, not production roots.

Unsigned, wrong-key, revoked and code-modified EFI images must produce OVMF's
explicit LoadImage rejection without entering the image. Disabled Secure Boot
and unprotected variable flash must produce explicit probe refusal. A timeout,
emulator setup failure, or a loaded image returning an error does not satisfy
firmware rejection.

`//secure/monitor:svm_probe_test` runs actual VMRUN/VMMCALL instructions under
x86 TCG and checks a forbidden nested-page access, port-I/O interception, MSR
interception, and nested-VMRUN interception. Its reusable entry routine switches
all software-owned general registers, x87/SSE state, and VMLOAD/VMSAVE state.
The probe alternates two independent diagnostic domains 128 times, checking
register continuity. It uses the development Multiboot adapter and neither runs
BexOS/Trusty domains nor publishes secure product-boot evidence. Full extended
CPU state beyond x87/SSE remains unfinished. Physical DMA assignment and four-CPU
BexOS execution are covered by the later combined checkpoint above. The probe
also executes NPT read-only/NX/revocation checks, NMI preemption of a
CLI/infinite-loop guest, virtual IRQ masking and delivery, and protected-mode/SIPI entry.

`//lib/secure_monitor_abi` defines an architecture-checked version-1 register
contract. The monitor registration table bounds shared RAM, checks caller
authority and domain ownership, rejects overlaps, and never reuses revoked
handles during a boot. Buffer leases validate offsets, lengths and access.
Actual guest VMMCALL tests verify capability and range rejection, registration,
unregistration, response delivery and stale-handle rejection. This table is
connected to the BexOS provider and Trusty's private bounce-buffer adapter. Its
normal-world secure-product acceptance is still pending; no guest physical
address is directly mapped into Trusty.

`secure/monitor/src/replacement.rs` coordinates registered image buffers and
Trusty A/B lifecycle events. An audit removed its placeholder digest, fabricated
candidate-health default, and unconditional successful commit callback. Staging
now requires an explicit trusted verifier that snapshots and authenticates the
image; the default verifier returns `Unsupported`. The coordinator itself no
longer dereferences integer host addresses. Update requests require the configured
update-owner domain as well as sharing authority, including activation requests.
Live activation returns `Busy` until the product owner reports preparation,
actual switch, secure-service readiness, and a successful persistent commit.
Repeated polling cannot restart the preparation deadline. Reboot activation
returns `Unsupported` until a persistent boot-selection backend exists.
Six coordinator regressions exercise these boundaries using an explicitly named
test verifier. They do not execute a candidate or authenticate firmware. Product
verification, startup, storage snapshot/catch-up, writer fencing, actual rollback,
and the resident monitor replacement nucleus are still missing. The Trusty
provider explicitly refuses monitor-component images instead of dispatching them
through the Trusty opcode, refuses cross-architecture requests, and validates
image-size and generation representability before reading the candidate bytes.

`//boot/efi:authenticated_monitor_test` now boots the signed EFI image through
the monitor probe in one QEMU instance. The EFI Authenticode signature covers
the embedded monitor ELF and generated load descriptors. The loader reserves
the payload's complete memory span, including BSS, before copying it, obtains
the memory-map key and exits boot services before entering the monitor. Tests
reject modified embedded monitor bytes, disabled Secure Boot, unprotected
variables and unavailable SVM. This is authenticated diagnostic execution;
the product Trusty/BexOS verification and execution chain is still missing.

### Actual Trusty execution under the monitor

The new `//secure/monitor:trusty_domain_boot_test` runs the validated saved x86
Trusty ELF inside an SVM domain with a relocated 256 MiB RAM bank. The bounded
ELF loader validates architecture, ranges, overlap and executable entry before
copying segments or zeroing BSS. The reusable NPT builder supports bounded 4 KiB
and 2 MiB mappings across full domain RAM, without mapping device apertures.
Guest instruction inspection walks only assigned RAM and rejects invalid page
entries. Guest MSR writes cannot install noncanonical VMLOAD state.

`//secure/monitor:trusty_acceptance_domain_boot_test` executes the real saved
acceptance firmware twice against persistent RPMB state, checking storage
round trips/generations, AuthMgr, KeyMint, Gatekeeper, AVB and orchestrator
acceptance. The current runtime uses a private HPET clock, root-owned NMI
watchdog, virtual xAPIC, and virtual legacy PIC/PIT. It intercepts device MMIO
and port accesses; guest PIC/PIT writes no longer modify physical controllers.
A regression during this conversion stalled service progress; implementing
Trusty's PIC initialization, PIT status latching/calibration and timer modes
restored the standalone acceptance pass (18.4 s, 21.438 s for the three-target
run with monitor host tests and SVM probe). IRQ delivery is exercised by actual
Trusty timer handlers and by a dedicated hardware IF/shadow test.

`//boot/efi:authenticated_trusty_domain_test` adds the signed EFI loader,
SMM-protected enrolled variables, exact embedded monitor/Trusty authentication,
and explicit tamper/disabled/unprotected/no-SVM rejection. The loader reserves
each domain RAM bank before ExitBootServices. RPMB attaches through QMP after
verified monitor entry; OVMF serial probes are discarded before attachment.
This avoids both RPMB frame corruption and an observed QEMU 11.1 socket
reconnection abort. Every helper and guest is reaped by its owning harness.
`//boot/efi:authenticated_trusty_acceptance_domain_test` adds two authenticated
boots against the same RPMB state. It passed in 31.3 s before PIC/PIT conversion;
the converted EFI path subsequently passed in 31.0 s, then in 30.4 s after formatting. These targets consume saved
firmware and do not implicitly rebuild it.

These are actual isolated Trusty execution targets, but **not the integrated
BexOS product**. They run a single Trusty vCPU and preserve the pinned firmware's
TCG development hypervisor compatibility interface. Saved firmware still uses
fake RNG/HWKEY providers and logs unavailable KeyMint authentication-token key
initialization; passing the existing acceptance client does not resolve those
security limitations. Normal-world TIPC, the BexOS verification/AVB approval
chain and actual live Trusty/monitor replacement remain outstanding. Combined
four-vCPU scheduling and IOMMU/device assignment are covered by the newer
checkpoint above. No secure product boot
flags are fabricated by these runtime targets.

The shared secure-runtime slot coordinator now separates candidate generation
from committed generation. It enforces the 30-second preparation budget and
150-ms quiescence-to-service-readiness budget using owner-supplied monotonic
time, including a timer hook for silent candidates. Health confirmation leaves
the old owner retained until the storage callback confirms commitment. Unknown
or interrupted commit outcomes prohibit rollback until authenticated storage
recovery resolves the outcome. These are coordinator tests; no firmware writer,
actual live activation, persistent-state fencing or reboot recovery backend is
installed by this change.

QEMU matrices include WASM, Brush, ELF/TLS, fault isolation and architecture
replacement wrappers. Both the maintained-inventory test and a repository-wide
Bazel query check enforce membership for QEMU-tagged tests, including direct
test declarations. The repository-wide check explicitly rejected a temporary
direct QEMU test in an unlisted package; that fixture was removed. Integrated
x86 scenarios retain their
explicit unavailable result; they do not select the software TEE implicitly.

The existing `third_party/linux_nvme` modifications are preserved.

### Latest focused checkpoint

After `bazel run @rules_rust//:rustfmt`, the following 11 targets passed uncached
under `-c opt --config=x86_64 --test_tag_filters= --nocache_test_results` in
129.614 s (2026-09-06, same base revision plus working changes):

- `//boot/efi:secure_boot_test` (11.2 s)
- `//boot/efi:authenticated_monitor_test` (8.7 s)
- `//boot/efi:authenticated_trusty_domain_test` (12.3 s)
- `//boot/efi:authenticated_trusty_acceptance_domain_test` (30.4 s)
- `//boot/efi:embed_monitor_tests` (1.0 s)
- `//secure/monitor:monitor_tests` (0.7 s)
- `//secure/monitor:svm_probe_test` (0.3 s)
- `//secure/monitor:trusty_domain_boot_test` (0.8 s)
- `//secure/monitor:trusty_acceptance_domain_boot_test` (18.4 s)
- `//lib/secure_monitor_abi:tests` (0.5 s)
- `//secure/orchestrator:tee_slots_tests` (0.4 s)

The authenticated-shell regression now again includes both users, wrong-password
rejection before and after authentication, disabled-user rejection, separate
provider processes, and retaining the authenticated identity across debugd
replacement. Preinstalling the fixture no longer bypasses those checks. Its uncached ARM
rerun passed in 465.4 seconds, together with `//secure/monitor:monitor_tests`,
`//lib/secure_monitor_abi:tests`, and `//secure/orchestrator:tee_slots_tests`
under `-c opt --config=aarch64 --test_tag_filters= --nocache_test_results
--keep_going` (1118.173 seconds including a fresh build). The earlier
reduced-coverage pass below is historical only.
This restoration does not change the explicitly requested ARM Brush skip.

### Combined-domain bring-up and device isolation

`//secure/monitor:dual_domain_development_boot_test` first reached four BexOS
CPUs, virtual HPET/IOAPIC delivery, idle secondary schedulers, ring-3 appd, and
Trusty RPMB readiness in 15.5 seconds. The vCPUs have separate saved register
contexts; Trusty and BexOS have disjoint NPT-backed RAM banks. Per-domain LAPIC,
IOAPIC, HPET, PIT/PIC, CMOS, and UART state replace guest access to physical
controllers. Intercepted HLT now waits for a deliverable guest interrupt.

The longer diagnostic initially reported Trusty service acceptance after BexOS
appd had failed device readiness. That 129.0-second result is **not** combined
system acceptance. Both runners now reject `appd: boot failed:` explicitly.
The development and combined acceptance targets now require debugd and the
normal-world disk/persistence boot marker as well. Those stronger gates are
being implemented with assigned PCI devices and must be rerun before claiming
success.

The VT-d hardware probe `//secure/monitor:svm_dma_probe_test` passed uncached in
1.0 second, alongside `//secure/monitor:monitor_tests` (0.4 seconds), under
`-c opt --config=x86_64 --test_tag_filters= --nocache_test_results` (4.642 seconds
elapsed). It enabled DMA and interrupt remapping, executed real QEMU EDU DMA
reads and writes through the assigned bank, and required fault records naming
the requester and target plus unchanged protected bytes for attempted writes
to monitor and secure-domain memory. This establishes the hardware backend,
not full product device assignment. Register definitions were checked against
[QEMU's VT-d backend](https://github.com/qemu/qemu/blob/master/hw/i386/intel_iommu_internal.h)
and the DMA transaction fixture follows [QEMU EDU](https://www.qemu.org/docs/master/specs/edu.html).

The initial authenticated combined-domain tests rejected execution because
OVMF could not reserve the fixed normal-world RAM bank in a 2 GiB machine.
Their fixture now supplies 3 GiB and the real normal-world device set; the
reruns are pending. The embedded EFI payload remains bounded at 128 MiB and
all monitor/domain reservations are checked for overlap. These targets still
use BexOS development evidence and are not the product verified-boot/Trusty
rollback approval gate.

## Latest matrix results (2026-09-06)

- ARM: 31/31 selected E2E targets passed uncached, 5576.143 seconds elapsed.
  The exact expanded target list appears in the log at
  `/tmp/bexos-01a076d8-arm-matrix-31.log`. It is the maintained ARM matrix minus
  `//testing/e2e/qemu/bexfs:brush_shell_e2e_test_aarch64`, explicitly skipped by
  the user. This includes WASM, ELF/TLS, faults, AuthMgr, the Trusty security
  stack, storage isolation, and normal-world replacement scenarios.
- x86 development: 32/32 passed uncached, 3397.149 seconds elapsed using
  `bazel --output_base=/tmp/bexos-01a076d8-checks test -c opt --config=e2e
  //testing/e2e/qemu:x86_64_development --test_tag_filters=
  --nocache_test_results --keep_going`.
- The ARM shell scenario passed in 237.5 seconds with a preinstalled native
  shell fixture and system-session debugd transplant. That historical fixture run skipped the
  authenticated-user shell checks. AuthMgr/security-stack tests do not establish
  that missing shell-specific coverage. This is not a full unchanged ARM matrix.
- Standalone saved x86 Trusty boot and acceptance passed uncached on the latest
  rerun: 2.0 and 154.8 seconds respectively, 158.593 seconds elapsed. Command:
  `bazel --output_base=/tmp/bexos-01a076d8-checks test -c opt --config=e2e
  //third_party/trusty:x86_64_boot_test //third_party/trusty:x86_64_acceptance_test
  --test_tag_filters= --nocache_test_results`.
- The monitor coordinator audit postdates those matrix runs. Its three focused
  targets (`//secure/monitor:monitor_tests`, `//lib/secure_monitor_abi:tests`,
  `//secure/orchestrator:tee_slots_tests`) passed uncached under x86 selection in
  5.703 seconds. Temporary debug-client upload logging was then removed.

After the coordinator audit, `//secure/orchestrator:tee_slots` was extracted as
an allocation-free shared library, preserving the orchestrator's public exports.
The monitor now includes replacement coordination in its bare-metal build rather
than hiding it behind a host-only conditional. The first build exposed the
old allocator dependency; the extracted library fixed that build failure.
Both x86 Trusty provider archives and `//secure/monitor:svm_probe` then built
successfully under `-c opt --config=x86_64` (3.320 seconds).

The following final focused checks passed after that extraction:

```sh
bazel --output_base=/tmp/bexos-01a076d8-checks test -c opt --config=x86_64 \
  //secure/monitor:monitor_tests //secure/monitor:svm_probe_test \
  //secure/orchestrator:orchestrator_tests //secure/orchestrator:tee_slots_tests \
  //boot/efi:authenticated_monitor_test //boot/efi:secure_boot_test \
  --test_tag_filters= --nocache_test_results
# 6/6 passed; 27.919 seconds elapsed.
bazel --output_base=/tmp/bexos-01a076d8-checks test -c opt --config=aarch64 \
  //secure/monitor:monitor_tests //secure/orchestrator:orchestrator_tests \
  //secure/orchestrator:tee_slots_tests --nocache_test_results
# 3/3 passed; 5.153 seconds elapsed.
```

The ARM image-update guest regression also passed after the provider validation
changes (85.9 seconds), alongside monitor and slot tests (94.712 seconds total).
`bazel run @rules_rust//:rustfmt` and `git diff --check` passed after the Rust
changes. These focused results do not establish the missing product backends.

These results use the base revision above plus working-tree changes; there is
still no accepted final revision and no integrated x86 product E2E pass.

## Focused validation

The following are focused results, not the plan's final acceptance gates.
Test result caching was disabled for QEMU runs.

The latest in-place resize change to the allocator and libc is newer than the
broad repository results below. Six allocator tests, all five focused
component/libc targets, and the ARM WASM guest pass. Brush still times out.
Earlier broad passes are not final-code acceptance for that change.

| Target | Result |
| --- | --- |
| Focused secure x86-selected run below | Passed 8/8 targets uncached, 26.910 s elapsed; authenticated monitor's five QEMU cases 9.1 s; EFI's seven cases 12.2 s; SVM/context/transport probe 0.3 s |
| Focused ARM-selected host run below | Passed 6/6 targets uncached, 5.140 s elapsed; includes all eight new slot-coordinator tests and existing Trusty C state tests; no ARM guest test was run |
| `//boot/efi:secure_boot_test` | Passed all seven boot cases, 11.4 s |
| `//third_party/ovmf:enroll_test` | Passed four variable-format/rejection tests, 0.3 s |
| `//secure/monitor:monitor_tests` | Passed VMCB/NPT unit tests plus monitor-owned Trusty replacement state, explicit verifier/lifecycle events, update-owner authorization, deadline enforcement, and commit-uncertain fencing, 0.5 s |
| `//secure/monitor:svm_probe_test` | Passed the expanded CPU/memory/device interception probe, 0.6 s |
| `//testing/e2e/qemu/wasm:wasm_e2e_test_aarch64` | Passed after in-place resizing, 289.1 s, including persistent clients and WASI descriptor recovery through replacement |
| `//testing/e2e/qemu/bexfs:brush_shell_e2e_test_aarch64` | Fails during component compilation after in-place resizing, 448.7 s. Appd ordinal 6 publication is repaired; full shell success is not established |
| `//apps/brush_shell:component_heap_tests` | Passed the real component lifecycle using a bounded 256 MiB BexOS heap after the free-list optimization, 99.7 s during the broad concurrent build; not a controlled performance comparison |
| `//lib/allocator:tests` | Passed four allocation/coalescing tests after the free-list optimization, 0.9 s in the broad run |
| `//lib/bexos_libc:internal_tests` | Passed nine tests, including actual ARM word-copy/fill and overlapping-move routines, 0.6 s on the native ARM host |
| Resize regressions: `//apps/brush_shell:component_heap_tests`, `//apps/brush_shell:component_tests`, `//lib/allocator:tests`, both libc targets | Passed 5/5 targets after resizing: bounded-heap Brush lifecycle 22.0 s, ordinary lifecycle 15.8 s; the six allocator tests also pass interleaved fragmentation/resizing with full arena recovery |
| Broad ARM-selected repository run below | Passed 104/104 tests, 735.354 s elapsed; QEMU tests excluded |
| Equivalent broad x86-selected repository run | 103/104 passed; `//services/wasm_runner:wasm_runner_tests` timed out at 300.1 s. Native stack sampling found the test thread waiting in Rust stack-handler cleanup; cause and reproducibility remain under investigation |
| `//services/wasm_runner:wasm_runner_tests` focused x86-selected rerun | Passed all ten uncached runs, 0.2–0.8 s each; the earlier intermittent timeout is not claimed fixed |
| `//:heart_transplant_coverage_test` | Passed archive coverage against the workspace manifests, 1.2 s, result caching disabled |
| `//testing/e2e/qemu:matrix_coverage_test` and `//testing/e2e/qemu:check_matrix` | Passed the extended coverage checks; a temporary direct QEMU test in an unlisted package produced explicit rejection |
| `//third_party/trusty:x86_64_boot_test` | Passed standalone saved-firmware boot, 10.3 s |
| `//third_party/trusty:x86_64_acceptance_test` | Passed standalone two-boot storage persistence, KeyMint, Gatekeeper, AuthMgr, orchestrator and AVB checks, 329.8 s; not integrated BexOS or secure product boot |

EFI/monitor checks used
`bazel --output_base=/tmp/bexos-secure-boot-bazel test -c opt ... --test_tag_filters= --nocache_test_results`.
ARM focused runs used `bazel test --config=e2e ... --nocache_test_results`.
The isolated output base allows small boot checks while a longer guest test
owns the ordinary Bazel server. QEMU version: 11.1.0, Apple Silicon macOS host.

The latest secure changes used `/tmp/bexos-01a076d8-checks`; the older
`/tmp/bexos-secure-boot-bazel` output base is no longer used by this task.
Latest focused invocations from this pass used `/tmp/bexos-01a076d8-checks`:

| Command group | Result |
| --- | --- |
| `//secure/monitor:monitor_tests //lib/secure_monitor_abi:tests //secure/orchestrator:tee_slots_tests` under `--config=x86_64` | Passed 3/3 targets, 1.811 s elapsed after fixing the monitor runtime tests |
| `//lib/boot:tests //tools/image:boot_handoff_tests //kernel:image_validation_tests` under `--config=x86_64` | Passed 3/3 targets, 4.001 s elapsed; v3 secure evidence remains accepted while v4 carries monitor handoff fields |
| `//services/teed:teed_tests //lib/tee_driver_trusty:tee_driver_trusty_tests //tools/qemu:qemu_tests //services/appd:appd_tests` under `--config=x86_64` | Passed 4/4 targets, 6.359 s elapsed |
| `//tools/qemu:qemu_tests //services/appd:appd_tests` under `--config=x86_64` before the monitor wrapper change | Passed 2/2 targets, 32.172 s elapsed |
| `bazel run @rules_rust//:rustfmt` | Passed after the final Rust changes |

Exact focused invocations:

```sh
bazel --output_base=/tmp/bexos-01a076d8-checks test -c opt --config=x86_64 \
  //boot/efi:authenticated_monitor_test //boot/efi:secure_boot_test \
  //boot/efi:embed_monitor_tests //lib/secure_monitor_abi:tests \
  //secure/monitor:monitor_tests //secure/monitor:svm_probe_test \
  //secure/orchestrator:orchestrator_tests //secure/orchestrator:tee_slots_tests \
  --test_tag_filters= --nocache_test_results
bazel --output_base=/tmp/bexos-01a076d8-checks test -c opt --config=aarch64 \
  //secure/orchestrator:orchestrator_tests //secure/orchestrator:tee_slots_tests \
  //secure/orchestrator/trusty:state_test //secure/monitor:monitor_tests \
  //lib/secure_monitor_abi:tests //boot/efi:embed_monitor_tests \
  --nocache_test_results
bazel run @rules_rust//:rustfmt
```

Logs are `/tmp/bexos-01a076d8-secure-focused-x86.log`,
`/tmp/bexos-01a076d8-secure-focused-arm.log`, and
`/tmp/bexos-01a076d8-rustfmt-secure-final.log`. All task-owned QEMU processes
were reaped; a separate checkout's ARM guest was left untouched.

After disabling Python bytecode writes in source-linked EFI test runfiles,
the three EFI targets passed again uncached: authenticated monitor 8.8 s,
seven-case EFI suite 11.7 s, embed validation 0.4 s, 21.817 s total. Log:
`/tmp/bexos-01a076d8-efi-cleanup-check.log`. The incidental bytecode file was
removed and no generated bytecode remains in `boot/efi`.

The Brush investigation found and removed Wasmtime's separate default 2 GiB
growth reservation. This is covered by the bounded-heap component test. It did
not resolve the ARM QEMU compilation timeout on its own. Disabling Cranelift
optimization also failed to resolve it and was reverted. The shared allocator's
adjacent-only coalescing and the ARM libc word-copy routines passed the WASM
guest regression but did not resolve Brush's compilation timeout. A temporary
QMP-enabled diagnostic run also timed out at 443.0 s. Two CPU samples showed
active Cranelift compilation and repeated heap allocation/free-list work. The
in-place resize change preserves the heap and allocation-header format. It
passes its focused regressions but did not resolve the Brush guest timeout.

The broad ARM-selected repository run passed with result caching disabled:

```sh
bazel test -c opt --config=aarch64 \
  //kernel/... //lib/... //services/... //drivers/... //tools/... //host/... \
  //secure/... //apps/brush_shell:tests \
  //testing/e2e/qemu:matrix_coverage_test //third_party/ovmf:enroll_test \
  --keep_going --nocache_test_results
```

These host tests do not substitute for an ARM guest E2E pass. The first broad
attempt exposed a WASM-only TTY library being selected for native compilation;
the target now declares its WASM CPU compatibility, and the complete run above
passed after that correction. Subsequent isolated checks use
`/tmp/bexos-01a076d8-checks` to avoid sharing an output base with another checkout.

## Firmware hashes

SHA-256 of the consumed saved bundles and unpacked OVMF artifacts:

| Artifact | SHA-256 |
| --- | --- |
| Trusty ARM standard `image.bin` | `730acb5d5c4dca520ce44c5706209c27bc9b5b94a29f4e9ed2ca726f8c073a1a` |
| Trusty ARM acceptance | `8b38fb3bdb00a7d973c7dad0e00aef406d4b6bfe8600971e369717c55301f218` |
| Trusty x86 standard, refreshed 2026-09-07 | `9cd20d59e82fb46e3c4fabc81f23a57d0950d0fb9bdf1fd85a767fe76b9eb7f8` |
| Trusty x86 acceptance with integrated boot readiness, refreshed 2026-09-07 | `ffdedd18857957f25a000775982cb47ac4eeb6f085d5d18a46adce8eb4af1db4` |
| Integrated x86 standard firmware, refreshed 2026-09-07 | `dad75e29c60206e0087c56b0ed362b95e511c42041b0a316c8bb8ecf271e23f7` |
| Integrated x86 acceptance firmware, refreshed 2026-09-07 | `71f14063fa4b832ae5a6b56f6811cbd8de0d630af72a74b3e01bb7cd1b801cf9` |
| Trusty x86 acceptance | `02dcc24ff5e1512479b0ebaad8a70340376218f6b344d0569312a63b340f3f9f` |
| OVMF secure code | `32807682a9e5c0e2d192ecb6077941d4be8bcfa6b9cd6e60830c3a9f48c0555a` |
| OVMF empty vars | `5d2ac383371b408398accee7ec27c8c09ea5b74a0de0ceea6513388b15be5d1e` |
| Latest signed EFI monitor diagnostic | `67efd2fb9f05ea3ca383bb6fc70e4336eb2723fae07869efcf2ec7bf6943d5a7` |
| Embedded SVM monitor diagnostic ELF | `f18a380d704a2469ff405e08f26a9de9ac0762ffa26d641e7f3d2ee9077b1587` |

OVMF is pinned to QEMU v11.1.0 published firmware. Compressed input hashes live
in `MODULE.bazel`. The pinned signer is osslsigncode 2.12, built by Bazel against
the host OpenSSL development libraries. No firmware source compilation was
used for these boot tests.

## Unfinished completion gates

- Complete maintained-matrix validation of the implemented x86 EFI/monitor/
  Trusty/BexOS chain and protected boot handoff. ARM pre-kernel Trusty AVB/RPMB
  approval remains pending; its existing bootstrap approves after kernel entry.
- Full secure-service acceptance through the normal-world x86 provider,
  including Gatekeeper, AuthMgr, orchestrator and concurrent operations beyond
  the proven product boot, KeyMint initialization and persistent storage.
- Product live Trusty A/B activation on ARM and x86, storage snapshot/catch-up,
  authoritative writer fencing, preserved clients, retryable interrupted
  operations, rollback and reboot recovery. The shared coordinator and x86
  monitor wrapper enforce the 30-second preparation and 150-ms readiness state
  budgets in unit tests, but no product end-to-end activation pass is claimed.
- Resident-nucleus x86 monitor replacement, complete CPU/device/transport
  state transfer and old-image reclamation.
- Product/cached-firmware replacement artifacts, complete rejection and
  isolation scenarios, live-replacement success/fault/recovery scenarios.
- Complete both saved integrated-firmware variants' guest and cache-boundary
  acceptance. The product now selects an image-independent saved monitor/EFI
  closure; explicit bring-up firmware probes and live-replacement artifacts
  still require further integration into the saved firmware workflow.
- Uncached final ARM/integrated-x86/development matrices, standalone Trusty
  acceptance, both architecture host suites, optimized image validation and
  transplant coverage on a final revision.

## Physical boards

Physical-board work remains future work. A board must provide an immutable
verification root and key/revocation provisioning, authenticated boot stages,
protected secure execution and memory, DMA isolation for every bus master,
authenticated persistent RPMB with rollback counters, and power-loss-safe
recovery slots. Live replacement additionally needs a resident recovery owner,
reserved transition memory, atomic durable commitment and measured service
readiness within the same deadlines. No board validation has been performed.
The long-term designs, now in `docs/rfcs`, remain applicable.
