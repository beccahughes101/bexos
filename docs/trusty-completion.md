# Trusty completion checkpoint — 2026-09-09

The ARM64/x86_64 live Trusty replacement plan is **not complete**. Neither
product currently executes a live replacement of Trusty/LK and its TAs.
The existing x86 monitor-policy replacement and Trusty reboot activation are
separate capabilities. The secure-stack E2E results below do not exercise live
Trusty replacement.

## Implemented foundations

- Firmware A/B storage authenticates against its execution owner's architecture
  instead of hardcoding x86. Boot-selection records bind requests, state and
  acknowledgements to architecture. ARM reserves the unsupported monitor
  component at its initial identity. Existing x86 wire records remain compatible.
- The orchestrator's protected boot-selection endpoint is enabled in ARM source.
  Both architectures prevent legacy journal writes from bypassing an existing
  boot-selection transaction. This does not provide ARM's missing resident
  transport, firmware disk owner or pre-kernel selection integration.
- `//secure/orchestrator:tee_slots` includes a separate live coordinator,
  single-writer gate and canonical protected-state codec. They track complete
  storage-write outcomes, imported watermarks, required service readiness,
  transport epochs, original deadlines, and uncertain durable commitment.
  Candidate write authority is granted only after a successful commit result.
  The persistence callback binds the image, source/candidate owners, storage
  watermark and storage/transport epochs. A timed-out preparation with an
  unresolved source write stays fenced until that write is resolved; a late
  resolution can then finish the rollback without replaying the mutation.
  The owner can recover the unresolved write ticket from the protected record
  after losing the original in-memory caller, without submitting the write again.
  These libraries require actual execution/storage adapters; they do not start
  Trusty, copy TA state, or intercept product RPMB traffic by themselves.
- Firmware control has capability, migration-ABI and transport-generation query
  fields. The current x86 nucleus advertises Trusty reboot and monitor live/reboot
  activation, with migration ABI zero. It does not advertise live Trusty.
  The provider checks the requested component's own generation and matches
  both component and generation before adopting a transaction report. ARM's
  transport ABI number is no longer reported as its firmware generation.
  Capability-aware owners can explicitly refuse each mode. For legacy owners
  that reject the new query field, previously supported reboot/monitor requests
  still reach their existing activation authorization; live Trusty requires an
  explicit capability and has no legacy fallback.
- The Trusty orchestrator TA exposes build-time Trusty generation, migration
  ABI and fixture mode through additive internal query commands. The live
  coordinator consumes a typed candidate report and rejects stale generations or
  migration-incompatible candidates before publishing any preparation state.
  Its protected recovery record now carries the migration ABI, so interrupted
  recovery does not silently assume compatibility after restore.
- Trusty firmware builds now use a shared prototxt image config for ARM and
  x86. Both architectures have generation-2, generation-3, incompatible-state,
  fault and hang configs and Bazel bundle targets for standard and AuthMgr
  acceptance firmware. The fault/hang fixtures are implemented inside the
  orchestrator TA with user-space-safe trap/hang behavior. These artifacts are
  available as build outputs; product live replacement does not yet consume them.
- A pinned Bazel QEMU build adds secure-only transition memory. Its prototxt
  generates shared C, Rust and linker geometry. A bare firmware probe checks
  S-EL2 entry and secure/non-secure memory access. This platform is not yet
  selected by the maintained ARM product and the probe is not a replacement test.
- Usersd now retains its pending TEE reply flag with the bound channel and
  KeyMint/Gatekeeper session IDs during service transplantation. After an RPC
  timeout it drains the old reply before submitting another operation, without
  replaying the timed-out mutation. The version-2 provider record rejects older
  records that cannot establish the queue state, malformed flags, and trailing
  data before changing the destination. This is normal-world RPC ordering, not
  the missing transport-generation rebinding across a Trusty replacement.

## Validation

The complete ARM-selected optimized host invocation on 2026-09-09 finished
**196/197 targets passed**, **750.388 s**:

```sh
bazel test -c opt --config=aarch64 --build_tests_only --keep_going //... --nocache_test_results
```

The sole failure was `//device/virtual/qemu/input_test:product_config_test`.
`validate_product.sh` permits graphics bundles only for `workstation`, while
the existing `input_test` product also enables graphics. This unrelated
validator is unchanged. The log is `/tmp/bexos-trusty-arm-host.log`.
The run preceded the final stale-generation guard and explicit owner-memory
reservation; the focused checks below cover those final edits.

The corresponding x86-selected command finished **196/197 targets passed**,
**1616.345 s**, with the same unrelated validator failure:

```sh
bazel test -c opt --config=x86_64 --build_tests_only --keep_going //... --nocache_test_results
```

Its log is `/tmp/bexos-trusty-x86-host.log`. This run preceded the final
coordinator commit-record and late write-resolution edits; it is not final-tree
acceptance for those changes.
Both broad runs also preceded the usersd RPC-ordering changes and recovery-ticket
accessor. Their targeted checks are recorded separately below.

Focused x86 checks passed **11/11**, **11.652 s**, and built the updated
driver and resident nucleus. Focused ARM checks passed **13/13**,
**224.959 s**, and built the ARM driver. These supersede earlier focused runs
but precede the recovery-ticket accessor and usersd changes described below.
Both include the coordinator, architecture-bound recovery, protocol and
optimized image checks. ARM also includes the generated-layout checks and the
rebuilt QEMU probe, which passed in **1.3 s**.

```sh
bazel test -c opt --config=x86_64 \
  //secure/orchestrator:live_tests //secure/orchestrator:tee_slots_tests \
  //secure/orchestrator/trusty:boot_selection_test \
  //lib/secure_firmware:tests //lib/secure_firmware:store_tests \
  //lib/secure_firmware:selection_wire_tests //lib/secure_monitor_abi:tests \
  //lib/trusty_boot:tests //lib/trusty_boot:recovery_tests \
  //secure/monitor:monitor_tests //kernel:image_validation_tests \
  //lib/tee_driver_trusty:tee_driver_trusty_shared \
  //secure/monitor:external_nucleus_product --nocache_test_results
bazel test -c opt --config=aarch64 \
  //secure/platform:memory_probe_test //secure/platform:layout_test \
  //secure/orchestrator:live_tests //secure/orchestrator:tee_slots_tests \
  //secure/orchestrator/trusty:boot_selection_test \
  //lib/secure_firmware:tests //lib/secure_firmware:store_tests \
  //lib/secure_firmware:selection_wire_tests //lib/secure_monitor_abi:tests \
  //lib/trusty_boot:tests //lib/trusty_boot:recovery_tests \
  //secure/monitor:monitor_tests //kernel:image_validation_tests \
  //lib/tee_driver_trusty:tee_driver_trusty_shared \
  --test_tag_filters= --nocache_test_results
bazel run @rules_rust//:rustfmt
bazel run //testing/e2e/qemu:check_matrix
```

Focused logs are `/tmp/bexos-trusty-x86-focused.log` and
`/tmp/bexos-trusty-arm-focused.log`. Rust formatting passed. The repository-wide
matrix check **failed** on four existing scenarios missing from maintained
matrices: `//testing/e2e/qemu/graphics:dioxus_smoke_aarch64`,
`//testing/e2e/qemu/graphics:dioxus_smoke_x86_64`,
`//testing/e2e/qemu/preferences:preferences_e2e_test_aarch64`, and
`//testing/e2e/qemu/preferences:preferences_e2e_test_x86_64`. Those unrelated
memberships are unchanged. The new secure-platform probe is included in
`//testing/e2e/qemu:firmware_acceptance`.

SHA-256 of artifacts consumed by the final focused checks (the store fixtures
are synthetic signed ELF inputs, **not running Trusty successor images**):

| Artifact under `bazel-out/darwin_arm64-opt/bin/` | SHA-256 |
| --- | --- |
| `third_party/qemu/qemu-system-aarch64` | `d7ab026b0ff9322539968e8ff021e0481df54bd1d480bd3425530e562b4a58b3` |
| `secure/platform/memory_probe.bin` | `e06cd68742167ee4ccc607a2434cca98f0733bf1530127afcb765d30e90c647e` |
| `lib/secure_firmware/storage_test_aarch64.fw` | `abc3faebaea2354cd1738ad4adf820eb8f0398cc8913e617955ed6e568c9b5ec` |
| `lib/secure_firmware/storage_test.fw` | `ed7f2022613df353f21691e10bba715a9a79eb91bc1e3ac9b39a8cee953ed01a` |

The ARM standard firmware refresh passed in **623.918 s**:

```sh
bazel run -c opt --config=aarch64 //third_party/trusty:refresh_image
```

The consumed, gitignored `third_party/trusty/image.bin` SHA-256 is
`614ce852d5c8a9ed29d9eeb8adc04d3e647c20ab77081cf466a80f19aadd0bb6`.
This builds the ARM boot-selection TA changes into the actual saved firmware.
The other five affected saved firmware variants have not been refreshed.

Two ARM secure-stack attempts before the final usersd transport changes failed:

```sh
bazel test --config=e2e //testing/e2e/qemu/trusty:trusty_security_stack_e2e_test_aarch64 --nocache_test_results
```

The first (**130.1 s** test time) passed KeyMint and secure-endpoint smoke checks
but returned usersd `Storage` (`-5`) for a wrong password, instead of
`AccessDenied` (`-4`). The diagnostic rerun (**475.5 s**) passed password
rejection/rotation, reboot persistence and AVB negatives, then failed its final
RPMB-corruption assertion: it expected a critical-TA crash, while the signed
BL33 verifier instead printed `Trusty rollback state unavailable; refusing
execution`. The test now requires that fail-closed verifier message, bad MACs,
and no normal-world startup. The intermittent first failure is not claimed
fixed by the subsequent reply-ordering hardening. Logs are
`/tmp/bexos-trusty-arm-security.log` and
`/tmp/bexos-trusty-arm-security-diagnostic.log`.

After the final reply-ordering changes, usersd migration tests and userspace
tests passed **2/2** in both configurations (**10.180 s** ARM build/test,
**10.133 s** x86). Both the initial and replacement usersd executables built:

```sh
bazel test -c opt --config=aarch64 //services/usersd:usersd_tests \
  //lib/userspace:userspace_tests //services/usersd:usersd_elf \
  //services/usersd:replacement_elf --nocache_test_results
bazel test -c opt --config=x86_64 //services/usersd:usersd_tests \
  //lib/userspace:userspace_tests //services/usersd:usersd_elf \
  //services/usersd:replacement_elf --nocache_test_results
```

Logs are `/tmp/bexos-trusty-usersd-arm.log` and
`/tmp/bexos-trusty-usersd-x86.log`. The final Bazel rustfmt invocation passed.
These host checks establish provider-record validation and retention; they do
not inject a late TEE reply into a running product.

The ARM secure-stack run passed again in **183.8 s** after the ARM Trusty
SMC restart handling fix. The earlier updated-usersd run passed in **276 s**;
the latest run uses the same refreshed standard ARM firmware plus the final
normal-world driver, vfsd and harness changes. It covers KeyMint
algorithms/opaque keys and deletion, secure endpoint reachability, queued calls,
Gatekeeper password rejection and rotation, encrypted-user-filesystem unlock,
persistence over a QEMU/RPMB restart, AVB tampering, stale rollback floors and
RPMB MAC corruption. It does not perform a live Trusty update, explicit
token-expiry/throttling timing tests, or live service/kernel transplantation.

The product invocation is:

```sh
bazel test --config=e2e \
  //testing/e2e/qemu/trusty:trusty_security_stack_e2e_test_aarch64 \
  //testing/e2e/qemu/trusty:trusty_security_stack_e2e_test_x86_64 \
  --nocache_test_results
```

The latest log is
`bazel-testlogs/testing/e2e/qemu/trusty/trusty_security_stack_e2e_test_aarch64/test.log`.
SHA-256 values of the latest ARM run's generated artifacts (paths relative to
`bazel-out/`):

| Artifact | SHA-256 |
| --- | --- |
| `darwin_arm64-opt/bin/kernel/kernel.bin` | `fecd7e16313f79ae48ca8ac090891eb18d64db1156b96a4957991422ccfd9a9b` |
| `darwin_arm64-opt/bin/device/virtual/qemu/nongui/bootfs.img` | `00bbf531b2b99227945df9afd516b319be1993657d1e5df91b438de46c5074bd` |
| `darwin_arm64-opt/bin/device/virtual/qemu/nongui/vbmeta.img` | `fdc7bd60b537d81374b5e97a6a0bfd617f8f71f253c8cfde45c72243cc7592b3` |
| `darwin_arm64-opt/bin/device/virtual/qemu/nongui/boot_evidence.bin` | `2e4648b7026b25e1d5efc16761c6764c9c92d7be2e778d14a31821e5dc390d05` |

No full maintained matrix pass is claimed. Firmware source changes require a
saved-bundle refresh before product tests consume them; legacy activation uses
the fallback described above and has not received a new product activation run.

The same product invocation failed its integrated-x86 test in **2230.5 s**
(**2715.682 s** total invocation). First-boot KeyMint, endpoint checks, password
rejection/rotation, unlock and lock completed. The persistent reboot failed
before debugd readiness because `reclaimed physical pages reused=` and
`appd: guest persistence and disk-only application verified` were missing.
Post-reboot unlock/deletion and subsequent AVB/RPMB negatives were not reached.
The original missing-marker error discarded the collected COM1 diagnostics;
the QEMU harness now includes a bounded diagnostic tail. The failure's cause
is unresolved and is not classified as unrelated or fixed.

This run consumed the existing integrated standard firmware, not a refreshed
build of the updated nucleus. Consumed SHA-256 values (generated paths relative
to `bazel-out/`):

| Artifact | SHA-256 |
| --- | --- |
| `boot/efi/standard_firmware.bin` (workspace saved bundle) | `8a36bd3e2c47dfe4c3c130fedf5b33f640a392a14832ca09bcf4891ab0cf60ac` |
| `darwin_arm64-opt-ST-19ca675d1cd5/bin/kernel/kernel` | `b6fc4bbe03b501b78e6516e259deb053b0c30c2c5190d2075b842b90cd3363c9` |
| `darwin_arm64-opt-ST-b63e00f1a730/bin/boot/efi/cached/OVMF_CODE.fd` | `32807682a9e5c0e2d192ecb6077941d4be8bcfa6b9cd6e60830c3a9f48c0555a` |
| `darwin_arm64-opt-ST-b63e00f1a730/bin/device/virtual/qemu/nongui/bootfs.img` | `485f3a59a67a26e47893832001d27b641f58a7a5ea0511c61c9c9c5810115093` |
| `darwin_arm64-opt-ST-b63e00f1a730/bin/secure/monitor/product_payload.vbmeta` | `4f5b8180428b44f3340e583d31a39d8b4dd8c8545326bbb8ea4844d2e1e26188` |

After adding the protected recovery-ticket accessor, coordinator checks passed
**2/2** for ARM (**14.189 s**) and **2/2** for x86 (**29.334 s**):

```sh
bazel run @rules_rust//:rustfmt
bazel test -c opt --config=aarch64 //secure/orchestrator:live_tests \
  //secure/orchestrator:tee_slots_tests --nocache_test_results
bazel test -c opt --config=x86_64 //secure/orchestrator:live_tests \
  //secure/orchestrator:tee_slots_tests --nocache_test_results
```

Logs are `/tmp/bexos-trusty-ticket-{rustfmt,arm,x86}.log`. Formatting passed.
The product invocation had already built its images before this library-only
accessor was added; neither product installs this coordinator yet.

After the missing-marker diagnostic change, formatting and the QEMU host
harness passed (**1/1 target**, **40.529 s** invocation, **2.3 s** test):

```sh
bazel run @rules_rust//:rustfmt
bazel test -c opt //tools/qemu:qemu_tests --nocache_test_results
```

Logs are `/tmp/bexos-trusty-diagnostics-{rustfmt,host}.log`. This verifies the
host harness; the failed x86 product test has not been rerun with the additional
diagnostics. All QEMU/RPMB children from these invocations have exited.

After adding the shared Trusty image config, orchestrator image identity queries
and candidate migration-ABI checks, the focused protocol/coordinator tests
passed **3/3**:

```sh
bazel test -c opt //secure/orchestrator/trusty:state_test \
  //lib/trusty_client:trusty_client_tests \
  //secure/orchestrator:live_tests --nocache_test_results
```

The new firmware fixture targets analyzed successfully for all twenty ARM/x86
standard and AuthMgr-acceptance fixture bundles:

```sh
bazel build --nobuild \
  //third_party/trusty:aarch64_generation2_built_image \
  //third_party/trusty:aarch64_generation3_built_image \
  //third_party/trusty:aarch64_incompatible_state_built_image \
  //third_party/trusty:aarch64_fault_built_image \
  //third_party/trusty:aarch64_hang_built_image \
  //third_party/trusty:aarch64_generation2_acceptance_built_image \
  //third_party/trusty:aarch64_generation3_acceptance_built_image \
  //third_party/trusty:aarch64_incompatible_state_acceptance_built_image \
  //third_party/trusty:aarch64_fault_acceptance_built_image \
  //third_party/trusty:aarch64_hang_acceptance_built_image \
  //third_party/trusty:x86_64_generation2_built_image \
  //third_party/trusty:x86_64_generation3_built_image \
  //third_party/trusty:x86_64_incompatible_state_built_image \
  //third_party/trusty:x86_64_fault_built_image \
  //third_party/trusty:x86_64_hang_built_image \
  //third_party/trusty:x86_64_generation2_acceptance_built_image \
  //third_party/trusty:x86_64_generation3_acceptance_built_image \
  //third_party/trusty:x86_64_incompatible_state_acceptance_built_image \
  //third_party/trusty:x86_64_fault_acceptance_built_image \
  //third_party/trusty:x86_64_hang_acceptance_built_image
```

Representative generation-2 successor bundles built successfully on both
architectures:

```sh
bazel build -c opt //third_party/trusty:aarch64_generation2_built_image \
  //third_party/trusty:x86_64_generation2_built_image
```

SHA-256 of the generated representative bundles:

| Artifact under `bazel-bin/third_party/trusty/` | SHA-256 |
| --- | --- |
| `aarch64_generation2/image.bin` | `8468e0e517e5d568222984bbcb77b06067ba21f05c5a3b2b9b491f386067ed22` |
| `bundle_x86_64_generation2/image.bin` | `09f6229b4b22d35cc000b1a96638822474345a60caf76b5873c63a4952cd19ef` |

The generation-3, incompatible-state, fault, hang and acceptance bundles have
analysis coverage but were not all compiled.


### 2026-09-09 follow-up: x86 secure-stack rerun

A later x86 product rerun reached first-boot Trusty smoke checks and then failed
inside the initial Gatekeeper enrollment path. Extending debugd's proxied user
mutation budget from **360 s** to **600 s** showed that the outer proxy timeout
was not the root cause: usersd eventually logged `user filesystem unlock failed
uid=2001 error=Io`, and vfsd logged `vfs: RPC ordinal=51 timed out`. Ordinal 51
is `get_package_directory`; the timeout happened while `bexos.platform.storage_verify`
package resolution competed with the initial encrypted user-volume
format/mount path.

The current tree now keeps a vfsd cache of mounted immutable package roots. A
package-directory request mounts the archive once and later callers receive a
duplicated cached root handle. Package archive write/delete invalidates the
cached root, and vfsd migration version 5 includes the cached package roots as
migrated resources so heart-transplant support is preserved. Debugd now keeps
the same **600 s** bounded mutation budget used by the host debug client. The
QEMU harness also includes a bounded diagnostic tail when debug-boot marker
collection times out, and the RPMB-corruption assertion accepts the x86
fail-closed path where Trusty storage observes the bad MAC and halts as a
critical app before a later monitor rollback-state readback.

Focused validation after these changes passed:

```sh
bazel run @rules_rust//:rustfmt
bazel test -c opt \
  //services/vfsd:vfsd_tests \
  //services/debugd:debugd_tests \
  //secure/orchestrator/trusty:state_test \
  //lib/trusty_client:trusty_client_tests \
  //secure/orchestrator:live_tests \
  --nocache_test_results
bazel test -c opt //tools/qemu:qemu_tests --nocache_test_results
```

The focused service/coordinator run passed **5/5** in **12.039 s**.
`//tools/qemu:qemu_tests` passed after the diagnostic-tail change.

The integrated x86 Trusty secure-stack gate then passed in **1918.7 s**:

```sh
bazel test --config=e2e \
  --test_env=BEXOS_QEMU_DEBUG_BOOT_TIMEOUT_SECONDS=600 \
  //testing/e2e/qemu/trusty:trusty_security_stack_e2e_test_x86_64 \
  --nocache_test_results
```

The log is
`bazel-testlogs/testing/e2e/qemu/trusty/trusty_security_stack_e2e_test_x86_64/test.log`.
The run covers protected Trusty app discovery, KeyMint algorithms and secure
deletion, orchestrator/AuthMgr reachability, queued secure requests,
Gatekeeper password rejection and rotation, encrypted-user-filesystem unlock,
QEMU/RPMB restart persistence, tampered kernel/BootFS/policy/vbmeta/verifier
rejection, stale rollback floors, and corrupted RPMB MAC fail-closed handling.
It still does **not** perform a live Trusty update, explicit token-expiry or
throttling timing tests, or live service/kernel transplantation.

Consumed SHA-256 values for the x86 secure-stack run:

| Artifact | SHA-256 |
| --- | --- |
| `boot/efi/standard_firmware.bin` | `8a36bd3e2c47dfe4c3c130fedf5b33f640a392a14832ca09bcf4891ab0cf60ac` |
| `bazel-out/darwin_arm64-opt-ST-19ca675d1cd5/bin/kernel/kernel` | `b6fc4bbe03b501b78e6516e259deb053b0c30c2c5190d2075b842b90cd3363c9` |
| `bazel-out/darwin_arm64-opt-ST-b63e00f1a730/bin/boot/efi/cached/OVMF_CODE.fd` | `32807682a9e5c0e2d192ecb6077941d4be8bcfa6b9cd6e60830c3a9f48c0555a` |
| `bazel-out/darwin_arm64-opt-ST-b63e00f1a730/bin/device/virtual/qemu/nongui/bootfs.img` | `0853405bcd7f7e470ff5325e76b48db15f2e015f9f81cc8d1cf26815a3818413` |
| `bazel-out/darwin_arm64-opt-ST-b63e00f1a730/bin/secure/monitor/product_payload.vbmeta` | `1fac3b820b8b6ece9e4c6aa68abb97be0812f8b239e2f983b1961751af9606a3` |

Process audits after interrupted diagnostic attempts and after the passing run
found no task-owned QEMU/RPMB processes left running; an unrelated QEMU process
from `/Volumes/Cache/Bazel/bexos2` was observed earlier and left untouched.

A subsequent ARM rerun exposed an early boot failure where KeyMint
`SET_BOOT_INFO` returned `Storage` while Trusty repeatedly logged
`sm_queue_stdcall: cpu 0, std call busy`. The ARM QL-TIPC driver now handles
`SM_ERR_BUSY` by resuming the outstanding stdcall with `RESTART_LAST` before
retrying the new command. This prevents a fresh KeyMint command from repeatedly
re-queuing while the storage proxy work it depends on is still in flight. Both
architecture builds of `//lib/tee_driver_trusty:tee_driver_trusty_shared` passed
after this change, and the ARM secure-stack gate above passed afterward.

The x86 secure-stack regression gate is no longer blocked by the create-user
stall. The complete live Trusty replacement plan remains unaccepted because no
product yet starts a candidate Trusty instance, migrates TA/provider state, cuts
over within the live deadlines, or commits generation 1→2→3 replacement.

## Remaining implementation and acceptance

1. Install the shared coordinator in actual x86 and ARM execution owners. X86
   needs independent candidate Trusty banks/CPU/platform/transport state. ARM
   needs its signed S-EL2 owner, TF-A integration, protected SMC transport,
   interrupt/watchdog ownership and durable firmware storage/boot selection.
2. Implement secure-storage snapshot/catch-up and enforce the writer gate on
   every actual persistent mutation. Add the TA/provider import/export hooks,
   preserving per-boot KeyMint/Gatekeeper secrets, unexpired tokens, throttling,
   secure time, boot authorization, and AuthMgr measurement/authorization rules.
3. Rebind retained public sessions, reject stale replies and operation handles,
   resolve interrupted mutations, perform real service health checks, and
   reclaim scrubbed source/candidate memory after the appropriate outcome.
4. Consume the new signed standard and acceptance successor, incompatible-state,
   fault and hang artifacts in product replacement tests. Refresh all affected
   saved closures sequentially and verify their consumed hashes.
5. Pass actual product live generation 1→2→3, negative/isolation and interrupted
   recovery scenarios on both architectures, secure-service acceptance,
   service/kernel/monitor transplant regressions, and the complete uncached
   maintained host/QEMU/firmware matrices.

Preparation remains bounded to 30 seconds and cutover/service readiness to
150 ms. Physical boards, production provisioning, ConfirmationUI and additional
secure services remain future designs. Existing Linux submodule changes are
outside this work.
