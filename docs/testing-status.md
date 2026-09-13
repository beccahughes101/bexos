# Testing Status

## Local font architecture validation (2026-09-12)

RFC 0063's local implementation adds the `bexos.fonts.FontProvider` protocol,
the transplantable wave-5 `fontd` service, system and per-user indexes,
read-only shared-VMO clients, Inter/JetBrains Mono/Noto image packaging, and
provider-backed Parley shaping in scened and the native Dioxus host. Dynamic
OCI/pkgd discovery was disabled at this local-font validation milestone; RFC 0064
now adds a configured resolver path whose guest acceptance is tracked separately.
The focused host, migration,
manifest, disk-layout, and image-closure selection passes:

```sh
bazel test //services/fontd:fontd_tests \
  //lib/font_client:font_client_tests //lib/flatland_text:tests \
  //lib/dioxus_render:tests //lib/ui/runtime:tests \
  //services/scened:tests //services/scened:internal_tests \
  //services/scened:controls_tests //services/scened:input_config_tests \
  //services/wasm_runner:wasm_runner_tests \
  //apps/sysui:tests //apps/userui:tests \
  //services/appd:appd_tests \
  //:heart_transplant_coverage_test //tools/image:generate_gpt_disk_test \
  //testing/build/architecture:closure_test \
  //testing/build/architecture:bootfs_closure_test \
  //testing/build/architecture:workstation_bootfs_closure_test \
  //testing/build/architecture:emulated_bootfs_closure_test

bazel run @rules_rust//:rustfmt
```

The production build passes for the generated FIDL, service and replacement
archives, shared client/text/UI libraries, scened and WASM-runner consumers,
SysUI/UserUI/Dioxus packages, workstation and nongraphical bootfs images, and
the workstation NVMe disk. In particular, the 320 MiB STORAGE partition and
its BexFS payload assemble with the font packages and replacement archives:

```sh
bazel build //idl:fonts_fidl_rust \
  //services/fontd:fontd_archive //services/fontd:replacement_archive \
  //lib/font_client:font_client //lib/flatland_text:flatland_text \
  //lib/ui/runtime:runtime //lib/dioxus_render:dioxus_render \
  //services/scened:scened_elf //services/scened:replacement_elf \
  //services/wasm_runner:wasm_runner_elf \
  //apps/sysui:sysui //apps/userui:userui //apps/dioxus_demo:dioxus_demo \
  //device/virtual/qemu/workstation:bootfs.img \
  //device/virtual/qemu/nongui:bootfs.img \
  //device/virtual/qemu/workstation:qemu_nvme_disk_image
```

`//testing/e2e/qemu/graphics:boot_ui_aarch64` was launched with an extracted
Ubuntu `qemu-system-aarch64` 10.0.2 binary because QEMU is not installed in the
host image. Under software AArch64 TCG, the default 900-second functional budget
expired while appd was launching wave-6 services. A second run used the bounded
`BOOT_UI_TIMEOUT_SECONDS=1500` harness override and reached `fontd: ready`, appd
acceptance of fontd and scened, provider registration, Parley shaping, glyph
rasterization, the retained desktop-text cache, splash takeover, scened's ready
background, and styled desktop text presentation. The full aggregate target
still did not pass: its 1,500-second budget expired later while the existing
stored VirtIO-GPU replacement was pending. Both runs cleaned up QEMU. The live
font-backed graphical path is verified through presentation, but this is not a
complete graphics/transplant acceptance pass.

`//testing/e2e/qemu/sysui:sysui_e2e_test_aarch64` was also attempted, but it
could not build because the saved Trusty firmware fixture is absent; QEMU did
not start for that target. The standard x86 assembled-image validation was
attempted separately and stopped on the same missing saved-firmware prerequisite.
Consequently no live SysUI/UserUI or x86 graphical result is claimed.

## Native UI component stack validation (2026-09-12)

RFC 0062 implementation adds shared `//lib/ui` component crates, native
Stylo/Taffy document rendering in the WASM runner, `prefsd` theme-manager
delivery, and SysUI/UserUI adoption of retained documents. The focused build
selection for this change passes:

```sh
bazel build //lib/dioxus_dom //lib/ui/... //lib/flatland_style \
  //lib/flatland_layout //lib/wasm_runtime //services/wasm_runner \
  //services/prefsd //apps/dioxus_shared //apps/sysui //apps/userui \
  //lib/ui/theme:theme_archive
```

The matching focused host validation passes:

```sh
bazel run @rules_rust//:rustfmt
bazel test //lib/dioxus_dom:tests //lib/ui/core:tests \
  //lib/ui/button:tests //lib/ui/text_input:tests //lib/ui/theme:tests \
  //lib/ui/runtime:tests //lib/flatland_style:tests \
  //lib/flatland_style:cascade_tests //lib/wasm_runtime:wasm_runtime_tests \
  //services/prefsd:prefsd_tests //services/wasm_runner:wasm_runner_tests \
  //apps/sysui:tests //apps/userui:tests
bazel test //:heart_transplant_coverage_test
```

QEMU graphical acceptance remains covered by the existing SysUI/Dioxus smoke
targets and should be rerun when recording full product validation for this
stack; no new QEMU run is recorded in this entry.

## GitHub Actions CI setup (2026-09-11)

The new `Bazel CI` workflow runs on PRs, pushes to `main`, and manual dispatch.
It prepares six saved firmware bundles through Bazel, then runs separate ARM/x86
build/test jobs and the ARM, integrated x86, development x86, and firmware
acceptance E2E suites. See [CI](ci.md) for runner prerequisites and exact commands.

The repository-wide inventory check exposed ten existing scenarios outside the
maintained matrices. Preferences and SysUI suites, plus both Dioxus smoke tests,
are now included. `bazel run //testing/e2e/qemu:check_matrix` passes with no
uncovered runnable QEMU scenarios.

Local validation used Apple Silicon macOS, not a GitHub-hosted Linux runner:

- The Bazel-managed workflow validator passed; workflow/composite YAML parsing
  and CI shell syntax checks passed.
- Fixture checks verified all six firmware refresh invocations and acceptance
  selection, tar transport paths/modes and extraction, failed-refresh exit-code
  preservation through `tee`, and absence of a partial transport artifact.
  These fixtures do not constitute successful firmware compilation.
- Diagnostic collection preserved XML, logs, and undeclared outputs from both
  optimized and fastbuild configurations, including when `bazel-testlogs` was
  repointed. Missing-test-log collection also passed.
- The aggregate check accepted all-success prerequisites and rejected each
  failed, cancelled, or skipped prerequisite in fixture validation.
- `bazel run @rules_rust//:rustfmt` completed after supplying both
  `--host_linkopt=--ld-path=/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin/ld`
  and the corresponding `--linkopt` for this local validation. Without these
  overrides the native compiler could not launch its linker. No Rust source
  changes resulted; these macOS flags are not part of Linux CI.
- Both `bazel build -c opt --config=<architecture> //...` and
  `bazel test -c opt --config=<architecture> --build_tests_only //...` were
  attempted for ARM and x86. All four commands failed analysis in the matrix
  genquery because the native Trusty Rust repository does not export the
  referenced `rust_std-x86_64-unknown-linux-gnu` target on this host.
- All four E2E suite commands were attempted. They failed overall because the
  existing macOS C/C++ toolchain requests a missing
  `runtime_library_search_directories` variable while analyzing Cargo build
  scripts. The input-hotplug transport test passed in all three architecture
  suites, the ARM Venus Linux host fixture passed, and firmware acceptance ran
  three passing tests: EFI Secure Boot, Multiboot handoff, and standalone x86
  Trusty boot. These partial results are not full-suite passes.
- The actual firmware-preparation script was attempted locally and stopped at
  its first ARM refresh with the same missing toolchain variable. No transport
  archive was published. Post-run process inspection found no QEMU/RPMB helpers
  left running.

The native toolchain failures are outside the CI setup change; no test failures
are suppressed by the workflow. A GitHub-hosted cold-cache run, complete Linux
firmware refresh, and successful full build/test/E2E results remain unverified.

## I2C/SPI service validation (2026-09-09)

The I2C/SPI implementation adds generated FIDL and topology protobuf schemas,
`//lib/i2c_spi`, `i2cd`, `spid`, appd bus enum/matching support, scoped
registrar grants, deterministic fixture controls, and separately linked
replacement archives for both services. The focused host validation and package
build commands below passed in this working tree:

```sh
bazel test //lib/i2c_spi:tests \
  //services/i2cd:runtime_tests \
  //services/spid:runtime_tests \
  //services/appd:appd_tests

bazel test //:heart_transplant_coverage_test

bazel build --config=aarch64 \
  //services/i2cd:i2cd \
  //services/i2cd:replacement_archive \
  //services/spid:spid \
  //services/spid:replacement_archive

bazel build --config=x86_64 \
  //services/i2cd:i2cd \
  //services/i2cd:replacement_archive \
  //services/spid:spid \
  //services/spid:replacement_archive

bazel build --config=aarch64 //services/appd:appd //services/appd:replacement_archive
bazel build --config=x86_64 //services/appd:appd //services/appd:replacement_archive

bazel run @rules_rust//:rustfmt
```

The host suites cover scoped address/chip-select access, topology rejection,
I2C repeated starts and final STOP, SPI chip-select boundaries, FIFO ordering,
bounds, timeout recovery, lock expiry/disconnect, SPI configuration isolation,
injected failures, appd manifest/device-registry integration, migration of
queued work and pending replies, backend mutations, lease deadlines, malformed
records, and abort recovery. The deterministic backend is used only for service
behavior and replacement-continuity validation; it does not establish physical
bus timing or board support.

An AArch64 QEMU fixture for concurrent peripheral clients across rejected and
successful service replacements is still missing from the maintained e2e matrix.
A package build alone is not counted as runtime success.

## USB host stack validation (2026-09-09)

The USB host implementation adds generated USB FIDL, `//lib/usb_host`, xHCI
controller, usbd, USB HID, and USB BOT packages. The focused build selection and
package/appd integration build pass in this working tree, followed by passing
focused USB tests:

```sh
bazel build //idl:usb_host_fidl_rust \
  //lib/usb_host:tests \
  //drivers/d1/usb/xhci:tests \
  //services/usbd:tests \
  //drivers/d1/input/usb_hid:tests \
  //drivers/d1/storage/usb/bot:tests

bazel build //services/appd:appd \
  //drivers/d1/usb/xhci:xhcid \
  //services/usbd:usbd \
  //drivers/d1/input/usb_hid:usb_hid \
  //drivers/d1/storage/usb/bot:usb_bot \
  //drivers/d1:manifest_services_test

bazel test //lib/usb_host:tests \
  //drivers/d1/usb/xhci:tests \
  //services/usbd:tests \
  //drivers/d1/input/usb_hid:tests \
  //drivers/d1/storage/usb/bot:tests \
  //drivers/d1:manifest_services_test

bazel test //:heart_transplant_coverage_test \
  //lib/migration:migration_tests \
  //tools/qemu:qemu_tests \
  //testing/e2e/qemu:matrix_coverage_test

bazel build --config=x86_64 \
  //drivers/d1/usb/xhci:xhcid \
  //services/usbd:usbd \
  //drivers/d1/input/usb_hid:usb_hid \
  //drivers/d1/storage/usb/bot:usb_bot \
  //device/virtual/qemu/nongui:x86_64_boot_image

bazel build //tools/qemu:qemu_tests \
  //testing/e2e/qemu/usb:usb_host_smoke_test_aarch64 \
  //testing/e2e/qemu:matrix_coverage_test

bazel run @rules_rust//:rustfmt
```

Declared QEMU acceptance lives at
`//testing/e2e/qemu/usb:usb_host_smoke_test_aarch64`. On 2026-09-09 it built
and launched QEMU with `qemu-xhci,msi=off,msix=off`, `usb-kbd`, `usb-mouse`,
and `usb-hub`, but the guest did not reach the USB readiness markers before
the run was interrupted. Outstanding work is listed in [USB](usb.md).

## Lazy keychain rollout validation (2026-09-08)

The lazy-service implementation has focused host coverage for manifest
compatibility and validation, appd dormant-provider activation, authorization
before launch, bounded/coalesced pending binds, lifecycle state, timeout and
handle cleanup paths, `//lib/lazy_service` connection/keep-alive/idle-generation
behavior, and keychaind snapshot compatibility across active clients, pending
idle, volatile-store keep-alive, and older snapshot records. The focused Bazel
selection currently passes:

```sh
bazel test //services/usersd:usersd_tests //services/jobd:jobd_tests \
  //services/timed:timed_tests //lib/lazy_service:lazy_service_tests \
  //lib/userspace:userspace_tests //services/appd:appd_tests \
  //services/keychaind:keychaind_tests
```

QEMU lazy-keychain coverage has been added at
`//testing/e2e/qemu/elf:lazy_keychain_test_aarch64` and
`//testing/e2e/qemu/elf:lazy_keychain_development_test_x86_64`. The affected
QEMU test binaries build. During this rollout validation, the integrated AArch64
run reached the storage service wave but stalled before the lazy scenario in
package/archive startup. The x86_64 development run spawned QEMU and connected
the debug socket but did not emit the first appd/debugd boot markers before it
was stopped. The standard integrated x86_64 target currently fails analysis when
`//boot/efi:selected_saved_firmware` expands to no files. These are recorded as
unresolved QEMU harness/boot prerequisites for this working tree; they are not
lazy-keychain pass results.


## Scened repository validation (2026-09-08)

The scheduled-CPU, Rust/C allocator, compositor-window and sustained-session work
is complete within the repository boundary tracked in
[scened repository validation](scened-validation.md). The focused host selection
passed 45/45 Bazel targets. Native AArch64 and x86_64 sustained aggregates passed
all five 32-warmup/256-measured workload windows, and the nested Venus aggregate
passed all GPU/direct-path windows at its 800×600 fixture extent. The graphical
boot/kernel gate passed 7/7 targets after refreshing the local Trusty firmware
cache, and the consolidated schema 3 report is
`docs/scened-completion-2026-09-08.json`. Sustained tests have a separate
maintained `//testing/e2e/qemu:performance` matrix and per-workload deadlines.
Hardware VSYNC, zero-copy GPU composition, and hardware 1080p/120 Hz acceptance
remain pending.

## Main rebase validation (2026-09-07)

The x86 branch was rebased onto main `71882a0`. Both complete default test
selections passed: **144/144 targets** with the default ARM selection and
**144/144 targets** with `--config=x86_64`. Commands used
`bazel test --build_tests_only //...` and
`bazel test --config=x86_64 --build_tests_only //...`; QEMU-tagged tests remain
excluded from these default gates.

All **8/8 focused QEMU targets** passed under `--config=e2e`: kernel smoke on
secure ARM, secure x86 and development x86; graphics boot and transplant on ARM,
secure ARM and development x86; BL33 verification; and the external nucleus
product's rejection of unauthenticated entry. The complete runtime matrices
were not rerun for this rebase.

Bazel Rust formatting, the repository-wide E2E matrix inventory, and the
heart-transplant manifest/archive coverage check passed. Both nongui and
workstation x86 launchers built with `-c opt --config=x86_64`.

The merge preserves main's split QEMU products and the branch's secure boot
path. Product-local ELF metadata now signs each product's own BootFS and policy.
Boot handoff v5 retains the monitor prefix and adds graphics metadata, with
normalization for both earlier v4 layouts. The diagnostic nucleus uses an
optimized closure to fit its fixed resident RAM reservation. QEMU library and
test dependencies are shared. Secure ARM graphics now attaches BL33's boot RPMB
UART before the normal-world virtio endpoint, and stops on a BL33 fatal error.
The BL33 verification and external nucleus boot checks are registered in the
maintained firmware acceptance matrix.

## Runtime matrix status

The maintained runtime matrices are `bazel test --config=e2e
//testing/e2e/qemu:aarch64`, `bazel test --config=e2e
//testing/e2e/qemu:x86_64` and `bazel test --config=e2e
//testing/e2e/qemu:x86_64_development`. They run optimized guests with
`--keep_going`; individual scenario names select the architecture. Integrated
x86 now boots the real Trusty provider through cached authenticated EFI/SVM
firmware. Focused product and rejection checks pass; its complete final matrix
and live secure-runtime replacement remain unfinished. ARM Brush is the only
user-authorized scenario skip.

The 32-target Q35, 28-target ARM, and 89-target host results in
[multiarchitecture validation](multiarchitecture-validation.md) are historical
results, not acceptance of the current working tree. The maintained matrices
now also include previously omitted WASM, Brush, ELF/TLS, and fault scenarios.
Current focused results and unfinished gates are recorded in
[secure integration validation](secure-integration-validation.md).

Run `bazel run //testing/e2e/qemu:check_matrix` to discover runnable architecture
scenarios across all QEMU packages and reject missing matrix membership.
`//testing/e2e/qemu:matrix_coverage_test` additionally checks the maintained
inventory during ordinary Bazel tests. Scenario declarations after a package's
`qemu_suites()` call fail at loading time.

Historical full uncached runs on 2026-09-06 passed 31/31 selected ARM scenarios
(5576.143 seconds, Brush excluded by user request) and 32/32 x86 development
scenarios (3397.149 seconds). ARM debugd shell uses a preinstalled native fixture
and skips its authenticated-user shell checks; that coverage is still missing.
Subsequent monitor coordinator changes have focused host regression coverage,
not final integrated product acceptance. See the validation record above.

## QEMU split rebase validation

The graphics split now preserves the x86 development product, CMOS RTC,
manifest stamping, software-driver development disk, and separate COM1/debug0
transports. The development targets and E2E artifacts live under `nongui`.
The merged runner retains graphics launch policy and interrupt cleanup.

After resolution, 14 focused Bazel tests passed: the runner test target,
product identity/security checks under both architectures, both standard x86
BootFS checks, and the AArch64 nongui/workstation/emulated BootFS checks.
Both architectures' nongui/workstation launch targets build, as do
`nongui:run_emulated` and `nongui:virtual_x86_64_development` under x86.
`bazel run @rules_rust//:rustfmt` and whitespace checks passed. This validation
built launchers without running guest instances; it does not replace the
historical E2E/runtime results below.

## QEMU product split (2026-09-06)

The results in this subsection describe the original graphics split before
the x86 development changes were rebased into it. See [x86 support](x86_64-support.md)
for the current development-product behavior.

The QEMU E2E harness now selects `//device/virtual/qemu/nongui` artifacts.
`workstation` shares the same architecture-specific firmware and Trusty policy,
with native QEMU display and virtio GPU/keyboard/mouse configuration.

Verification results for the split:

- AArch64 nongui/workstation images and developer launchers build through Bazel.
- Product identity/bundle and secure-world tests pass for both products under
  both architecture configurations. Runner, assembly, config compiler, AVB,
  appd, and both architecture probes pass.
- BootFS architecture/assembly checks pass for AArch64 nongui, workstation,
  and nongui’s software-TEE variant, including core/component WASM validation.
- The AArch64 workstation runner reaches debugd readiness; its product-specific
  `:debugd -- health` reports `SERVING`. Serial evidence includes Trusty
  bootstrap, the signed Trusty driver, and `virtio-console: rpmb0 ready`.
- Ctrl-C exits the runner successfully and removes its owned QEMU/RPMB children,
  debug socket, and temporary writable state.
- `nongui:run_emulated` builds and starts the raw-kernel software-TEE path,
  but this run fails loading `teed` with `Elf(UnresolvedStrongSymbol)` before
  pivot. Inspection of the software-TEE shared library finds an unresolved
  `rust_eh_personality` import. The runner reports failure and cleans up its
  instance; software-TEE guest readiness is not verified.
- The x86 firmware cache was rebuilt successfully using
  `//third_party/trusty:refresh_x86_64_image`. Full x86 product compilation is
  currently blocked by the existing AArch64-only Wasmtime/BexOS runtime
  selection: the x86 build attempts Linux `helpers.c` and cannot find
  `stdlib.h`. This is separate from the QEMU product/configuration split.

The complete `bazel test --config=e2e //testing/e2e/qemu:all_architectures`
run executed all 27 AArch64 scenarios: **16 passed and 11 failed**. All 27 x86
scenarios failed to build at the Wasmtime issue above; x86 runtime repairs
remain outside this change.

Passing AArch64 scenarios cover SYS_STATE, app registry, tracing, user-storage
isolation, jobs, kernel/NVMe smoke, Trusty AuthMgr and security-stack acceptance,
app-package updates, debugd retained-state migration, kernel replacement,
NVMe/UART replacement, RPMB console replacement, and TEE image update policy.

| Failed AArch64 scenario (`_aarch64` suffix) | Observed failure |
| --- | --- |
| `archivefs_update_e2e_test`, `bexfs_update_e2e_test`, `debugd_update_e2e_test`, `teed_update_e2e_test`, `update_e2e_test`, `vfsd_update_e2e_test` | BexFS migration preparation timed out; later replacements in each chain were not reached. |
| `pci_update_e2e_test` | Required guest readiness markers were missing at the 600-second deadline. |
| `appd_update_e2e_test` | Appd replacement committed, then live registry access returned `app lifecycle list transport failed`. |
| `updated_update_e2e_test` | Persistent-client prelude returned `progress transport`. |
| `rpmb_teed_update_e2e_test` | Rejection rollback check returned `migration status transport`. |
| `debugd_shell_e2e_test` | The initial “no provider installed” assertion instead received a provider-launch failure. The pre-split product already installs `brush_shell`; the split preserves that content. |

The BexFS failure stage matches the previously recorded debugd-chain baseline
below, and the shell fixture's absent-provider assumption conflicts with the
pre-split product contents. The other runtime failures are recorded as observed;
this run does not establish whether they are intermittent baseline failures or
regressions. No guest runtime repairs or weakened assertions were introduced to
make this matrix pass. The BootFS validator was corrected to recognize existing
core and component WebAssembly artifacts alongside native ELF files.

`bazel run @rules_rust//:rustfmt` and `git diff --check` completed successfully.

Native-window visual inspection requires an unlocked macOS session; the session
was locked during this verification. A running native-UI QEMU command and guest
readiness do not substitute for visual inspection, guest rendering, or input
driver support. The historical matrix results below predate this split.

## Major Test Targets

Current Bazel test targets include:

- `//kernel/core:core_tests`
- `//kernel/smoke:...`
- `//lib/domain_association:domain_association_tests`
- `//lib/job_store:job_store_tests`
- `//services/appd:appd_tests`
- `//services/debugd:debugd_tests`
- `//services/keychaind:keychaind_tests`
- `//services/jobd:jobd_tests`
- `//services/netstack:netstackd_tests`
- `//services/netstack:netstack_tests` (alias for `netstackd_tests`)
- `//services/timed:timed_tests`
- `//services/traced:traced_tests`
- `//services/vfsd:vfsd_tests`
- `//services/powerd:powerd_tests`
- `//tools/qemu:qemu_tests`
- `//drivers/d1/storage/bexos/archivefs:archivefs_tests`
- `//drivers/d1/storage/bexos/archivefs:archivefs_async_tests`
- `//drivers/d1/storage/bexos/bexfs:bexfs_tests`
- `//drivers/d1/storage/bexos/bexfs:bexfs_async_tests`
- `//drivers/d1/storage/bexos/diskimage:diskimage_tests`
- `//drivers/d1/storage/bexos/diskimage:diskimage_async_tests`
- `//drivers/d1/support/linux/shim:linux_shim_tests`
- `//drivers/d1/support/linux/shim:linux_shim_std_tests`
- `//drivers/d1/storage/nvmexpress/nvme:nvme_async_tests`
- `//drivers/d1/storage/nvmexpress/nvme:nvme_tests`
- `//drivers/d1/bus/generic/pci:pci_root_bus_tests`
- `//drivers/d1/serial/arm/pl011:pl011_tests`
- `//tools/app_archive:app_archive_tests`
- `//tools/assembly:assembly_tests`
- `//tools/fidlc:fidlc_tests`
- `//tools/image:assemble_bootfs_test`
- `//tools/image:generate_gpt_disk_test`
- `//tools/image:bexfs_image_e2e_test`
- `//tools/bexctl:bexctl_tests`
- `//lib/app_archive:app_archive_tests`
- `//lib/app_registry:app_registry_tests`
- `//lib/crypto:crypto_tests`
- `//lib/debug_wire:debug_wire_tests`
- `//lib/trace:trace_tests`
- `//lib/keychain_store:keychain_store_tests`
- `//lib/migration:migration_tests`
- `//lib/redb:redb_tests`
- `//lib/update:update_tests`
- `//lib/userspace:userspace_tests`
- `//lib/trusty_client:trusty_client_tests`
- `//lib/tee_driver_trusty:tee_driver_trusty_tests`
- `//secure/orchestrator:orchestrator_tests`
- `//host/debug_client:debug_client_tests`
- `//host/trace_analysis:trace_analysis_tests`

## Coverage Areas

The current tests focus on:

- FIDL parsing, validation, capability metadata, and Rust backend generation;
- app manifest decoding and service broker permission/visibility behavior;
- permission-store separation for system/UID 0 records versus per-user grant
  records, UID-scoped redb snapshot/replace, and legacy nonzero-UID system-row
  scrubbing;
- optional permission request ABI coverage for explicit service/capability
  selectors and nonzero returned handles, plus broker coverage for exact and
  ambiguous capability selection;
- app registry version selection, exact active-pin checkpoint/persistent reopen,
  protected rollback clearing, probation fallback pruning protection, and
  digest validation;
- appd lifecycle policy decoding, deterministic crash-loop watchdog transitions
  for probation promotion, rollback, restart delay, counter reset, missing
  fallback, and watchdog checkpoint round trips;
- runner policy, ELF mapping, manifest-declared shared-library resolution,
  SONAME validation, dependency-cycle rejection, text-relocation rejection,
  runtime linker-data encoding, static TLS startup sizing, package image
  resolution, hierarchical VMAR image/library/TLS/stack construction, stack
  guards, W+X rejection, partial-load cleanup, and launch planning;
- startup waves, driver matching, parented device-registry validation,
  multi-instance D1 service publication, recovery exclusions, and appd
  preservation of driver topology/resource/recovery state;
- kernel core IPC, synchronous call/reply token lifecycle, fair-priority
  donation, memory/runtime records, scheduler/resource-group behavior,
  lazy anonymous zero-page reads, first-write page commitment, contiguous DMA
  pin materialization, PAC key snapshot preservation, resource-group
  memory/GPU accounting, and migration handoff models;
- PSCI contract coverage for AArch64 function IDs, version/feature decoding,
  signed return-code mapping, unsupported-return handling, and supported
  power-operation mapping;
- AArch64 hardening policy coverage for detected versus active BTI/PAC/
  speculation controls, entropy-gated PAC activation, inactive fallbacks, and
  branch-protected ELF artifact validation;
- app archive creation/verification and zstd paths;
- storage drivers, filesystem model behavior, and std/Tokio guest entry compile
  shape for NVMe, BexFS, archivefs, and diskimage;
- std/Tokio guest entry compile shape for trustd, debugd, and traced, trustd
  BootFS app root redb loading plus direct Ed25519 validation from the generated
  store path, traced runtime migration state preservation, and debugd async
  frame-handler coverage under a Tokio current-thread test runtime;
- netstackd static/DHCP/IPv6 config selection, FIFO frame sequencing and
  reconnect, UDP Ethernet+IPv4 packet encode/decode plus smoltcp UDP socket
  plumbing, typed DNS A/AAAA query/response caching, TCP metadata and stream
  bridge state, dual-stack resolver addresses, per-socket smoltcp checkpoint
  record preservation, and fail-closed heart-transplant validation for
  established TCP sockets without a restorable checkpoint;
- time ABI slew/seqlock conversion, kernel clock adjustment/vDSO handle rights,
  libc fast-path fallback, timed SNTP/NTS helper packet decoding, time-quality
  derivation, persisted state record round-tripping, plus std/Tokio entry
  compile shape and heart-transplant record preservation for config, quality,
  NTS association state, clients, in-flight slew, RTC provider state, and sync
  schedule;
- component-config v1/v2 parsing, assembly v2 emission, generated Rust binding
  output, appd config-override migration records, debug wire framing, config
  get/set/reset messages, trace wire messages, and host debug client behavior;
- trace shared-ring layout, category parsing/filtering, Perfetto/default and
  legacy BexOS FXT export, traced session/producer lifecycle, trace-session
  snapshot/restore support, debug-wire/host/CLI format selection, FIDL
  cross-library imports, and host trace-analysis assertions;
- normal-world crypto helpers, no-std/std redb adaptation, and alias-scoped
  keychain vault behavior, plus crypto/net library package/archive
  construction and shared-library client linker-data parsing;
- job-store deterministic record encoding, corrected manifest clamp behavior,
  durable/transient timebase handling, realtime anchoring, reboot
  reconciliation, package instance purging, constraint checks, and `jobd`
  runtime condition gating;
- provider watcher migration/notification coverage for powerd, netstackd,
  timed, and usersd, powerd provider absence/threshold/hysteresis/stale
  fail-safe/resource-cap policy, plus jobd watcher consumption, batching state
  migration, worker timeout/stop paths, and method-authorization checks;
- typed KeyMint, Gatekeeper, AVB, AuthMgr, storage, and orchestrator framing,
  malformed responses, driver session lifecycle, KeyMint opaque-blob and
  protected-operation routing, and per-operation usersd token brokerage;
- Gatekeeper enrollment, incorrect-password/throttling paths, authenticated
  UKEK recreation, five-minute expiry, invalidation, and heart-transplant token
  migration without lifetime extension;
- teed async backend/service behavior, std/Tokio entry compile shape, external
  ABI-v1 driver binding/fail-closed behavior, package-aware trusted-app state,
  and heart-transplant record preservation for sessions, update progress,
  connected clients, live activation, reboot-pending activation, and Trusty core
  update status;
- update metadata verification and `updated` restoration/commit of durable
  kernel/TEE generation floors through appd lifecycle controls;
- heart-transplant coverage validation through
  `//:heart_transplant_coverage_test`, which checks every shipped
  `HEART_TRANSPLANT` service/D1-driver prototxt has a Bazel replacement
  archive;
- product assembly and image helper scripts;
- QEMU runner coverage for secure-machine selection plus lifecycle-owned
  upstream RPMB proxy startup, `rpmb0` attachment, image reuse, and teardown;
- secure orchestrator model behavior, including Trusty dual-slot state, pending
  reboot activation, health confirmation, abort, monotonic generation
  enforcement, and rollback.
- kernel runtime IOMMU-domain ownership, domain-backed DMA mappings, and legacy
  pin denial after a process owns a device domain.

The tagged QEMU path boots the pinned firmware containing KeyMint, Gatekeeper,
storage, AVB, AuthMgr FE/BE, and the orchestrator, with the upstream RPMB proxy.
Physical-board secure boot/RPMB behavior is deliberately not claimed.

## Earlier Trusty integration baseline (2026-09-05)

Bazel rustfmt and all 85 repository test targets pass, including 114 kernel
host tests. The complete tagged QEMU suite passes all 25 targets. The final
run executed 15 affected targets and reused 10 unchanged passing results;
it completed in 4348.5 seconds. Uncached standalone AuthMgr acceptance passes
in 26.3 seconds. A repeated uncached appd QEMU run passes in 59.3 seconds with
both saved firmware hashes unchanged and no firmware compilation.

### Verified QEMU results

- Authenticated boot, protected TA discovery, KeyMint, Gatekeeper enrollment
  and password replacement, concurrent secure requests, and persistence across
  QEMU/RPMB restarts pass with the refreshed four-CPU standard firmware.
- Modified kernel, BootFS, policy, vbmeta, unauthenticated BL33, stale rollback
  indexes, and corrupt authenticated RPMB state are explicitly rejected before
  normal boot. Missing artifacts, unrelated errors, and timeouts cannot count
  as successful rejection.
- The full ten-service replacement chain passes in 841.2 seconds, including
  appd at 70 ms, teed at 34 ms, and BexFS at 97 ms. Every service cutover retains
  the 150 ms limit and verifies lifecycle health, registry state, continued
  client progress, and complete postcommit memory reclamation.
- Dedicated PCI, UART, NVMe, BexFS, ArchiveFS, VFS, debugd, updated, appd, teed,
  and RPMB-console replacement targets pass. Rejection, incompatible state,
  candidate failure, and timeout rollback preserve the original endpoints.
  Debugd completes a partial upload on its original host connection. NVMe
  verifies matching nonzero DMA addresses from the actual activation evidence;
  the harness waits for that evidence after kernel commit.
- Kernel replacement preserves running applications/processes, reclaims old
  memory, and rejects rollback. Kernel takeover currently uses CPU0; full SMP
  ownership migration remains a future design.
- User-storage write/growth/sync/reread, locked-user and cross-user denial,
  system-state persistence, application registry, trace export, application
  updates, jobs, and kernel/NVMe smoke targets pass.
- AuthMgr acceptance passes with the isolated four-CPU acceptance bundle.

Primary Trusty initialization drains before PSCI starts secondary CPUs and
again afterward; repeated QEMU boots pass without bypassing the RPMB gate.
Appd retains original hardware-resource ownership, updates replaced device
process descriptors, and reopens its persistent stores after handover. It
batches recovery/activation writes and flushes both backing volumes before
serving, avoiding repeated full-volume flushes during activation.

Process retirement atomically detaches source ownership and queues private
pages for bounded scrubbing before allocator release. Runtime snapshot version
14 preserves pending and partial reclamation. Commit, abort, legacy rejection,
and full/incremental checkpoint regressions pass. Retired threads cannot resume
and reconnect the storage proxy before replacement activation. Bounded direct
catch-up batches preserve the existing cutover limit.

### Current implementation

Teed services upstream storage-proxy requests through raw QL-TIPC while
secure calls wait. The lifecycle-owned RPMB helper connects through the D1
virtio-console driver on `rpmb0`. Driver resources and logical proxy sessions
support heart transplant, with interrupted transactions reported explicitly.
The embedded S-EL0 semihost RPMB implementation is removed. KeyMint performs
shared-secret negotiation; Gatekeeper uses authenticated tamper-proof storage;
AVB validates persisted rollback indexes.

Usersd requires durable storage before normal boot. Usersd and keychaind retain
separate bound VFS/TEE endpoints. A separate BexFS instance mounts user images
to avoid recursive synchronous storage calls. Missing permission databases mean
empty grants; actual mutations create durable databases. Installed applications
use their canonical archive paths and acknowledge startup before long I/O.

Kernel checkpoints preserve secure authority, DMA domains/mappings, scheduler
state, and shared VMO ownership. Retired threads are removed from scheduling,
and private pages remain kernel-owned until bounded maintenance scrubs them
before allocator reuse. BexFS
migration preserves the full shared block buffer and sparse file data. Host
coverage includes malformed migration acknowledgements, rollback, partial
resource cleanup, interrupted transports, and QEMU/RPMB child cleanup.

### Saved firmware and completed gates

Standard firmware contains seven functional TAs plus the four user-approved
upstream providers: hwcrypto, hwbcc, hwcryptohal, and system-state. Acceptance
adds its isolated service and client. Both retain assertions, omit embedded
symbol tables, and use size optimization; `lk.elf` remains in the bundles.
AuthMgr fixes include unbuffered Binder receive handling, a scoped FE HWBCC
allowlist, signed-payload validation, pinned AAD encoding, and client certificate
issuer linkage. Production manifests remain prototxt.

Explicit refresh saves gitignored `third_party/trusty/image.bin` and the separate
`authmgr_acceptance_image.bin`. QEMU extracts validated declared outputs and
keeps writable RPMB state outside the bundle across reboots. Repeated actual
runs after normal-world changes left both bundle hashes unchanged. The final
Bazel action query over all 64 QEMU targets found no action consuming either
firmware build script. The separate query of the ordinary QEMU run target
also found no firmware compilation action.
An interrupted refresh preserved the previous complete standard bundle.
Both four-CPU bundles remain unchanged after the full suite, standalone AuthMgr,
and the repeated uncached appd run. That repeated run executed only two local
actions. Final process inspection found no QEMU or RPMB helper children.

The required rustfmt, repository, full QEMU, standalone AuthMgr, and saved-image
reuse gates pass. Physical-board validation, full SMP ownership migration,
and future secure services remain outside this work.

The QEMU debug trace smoke is
`//testing/e2e/qemu/bexfs:debugd_trace_e2e_test`. It boots the standard QEMU
product, starts a debug-service trace through debugd, runs a health check,
exports the trace, and verifies the Perfetto-default trace artifact plus the
expected debugd event through the trace-analysis helper. This target is not run
as part of the focused non-e2e verification set.

The scheduling/SMP work is covered by `//kernel/core:core_tests` for CPU
ownership, affinity, cross-CPU scheduling decisions, EDF admission, tickless
deadline selection, resource-group memory/GPU accounting, synchronous IPC
donation, and runtime snapshot/incremental profile preservation. A broader
QEMU scheduling probe remains a manual/e2e validation step rather than a
replacement for those focused host tests.

## Useful Verification Commands

Run repository tests (the default tag filter excludes QEMU):

```sh
bazel test //...
```

Run the full QEMU suite and uncached standalone AuthMgr acceptance:

```sh
bazel test --config=e2e //testing/e2e/qemu:all_architectures --test_env=BEXOS_QEMU_LIVE_LOG=1
bazel test -c opt //testing/e2e/qemu/trusty:authmgr_acceptance_e2e_test --test_tag_filters=requires-qemu --test_env=BEXOS_QEMU_LIVE_LOG=1 --nocache_test_results
```

Verify saved-firmware dependency isolation:

```sh
bazel aquery -c opt 'inputs(".*(build_firmware.sh|build_acceptance.py)", deps(//testing/e2e/qemu/...))'
bazel aquery -c opt 'inputs(".*(build_firmware.sh|build_acceptance.py)", deps(//device/virtual/qemu/nongui:run))'
```

Both queries return zero matching actions. Bundle round-trip, malformed/missing
bundle, interrupted refresh, and child-process cleanup regressions are included
in the passing repository gate.

Run focused subsystem tests:

```sh
bazel test //kernel/core:core_tests
bazel test //services/appd:appd_tests
bazel test //lib/job_store:job_store_tests
bazel test //services/jobd:jobd_tests
bazel test //lib/trace:trace_tests
bazel test //services/traced:traced_tests
bazel test //host/trace_analysis:trace_analysis_tests
bazel test //drivers/d1/...
bazel test //tools/...
```

Run Rust formatting:

```sh
bazel run @rules_rust//:rustfmt
```

## Documentation Verification

This `docs` set was written from the checked-in Bazel targets, prototxt manifests, IDL files, and Rust module layout. When implementation changes, update the relevant page in this directory in the same change as the code/config.

Current versioning coverage includes manifest SemVer decoding, partial
package/version selector matching, multi-version registry records with active
pins and rollback targets, vfsd versioned archive/data paths, and appd
dependency resolution through registry-selected packages. Current SYS_STATE
coverage includes v2 two-record sequencing and v1 upgrade compatibility.

Current user-storage isolation coverage includes sparse BexFS holes,
partial-block writes, truncation allocation accounting, sparse namespace
persistence, encrypted DiskImage unwritten-sector zero reads, wrong-key
rejection, header/ciphertext tamper rejection, usersd U-KEK transfer, vfsd
locked-directory denial, and appd UID 0 system-data routing. The QEMU coverage
target is `//testing/e2e/qemu/bexfs:user_storage_isolation_e2e_test`; it builds
a signed probe app archive, creates and unlocks a user, writes recognizable data
through `/data`, triggers growth pressure, and verifies locked-user launch
denial.

Current domain-association coverage includes canonical domain keys, manual
protobuf round trip, malformed protobuf rejection, TTL expiration checks, redb
reopen behavior, and record replacement.

## Debug CLI and terminals

Focused targets cover the Clap command tree/reference, output and dispatch,
terminal restoration on an error, client frame bounds and partial responses,
credential validation, process metadata, and terminal state codecs:
`//tools/bexctl:bexctl_tests`, `//tools/bexctl:terminal_tests`,
`//host/debug_client:framing_tests`, `//host/debug_client:debug_client_tests`,
`//lib/debug_wire:debug_wire_tests`, `//services/debugd:debugd_tests`, and
`//lib/tty:tty_tests`, and
`//testing/e2e/qemu/bexfs:shell_fixture_tests`.

`//testing/e2e/qemu/bexfs:debugd_shell_e2e_test_aarch64` exercises the unbundled
terminal provider fixture, authentication, terminal byte/control paths, and
an active terminal across debugd heart transplant and rollback, final output
draining, lease expiry, and provider reuse after close. The architecture wrappers request the same
test closure on both guests; guests reporting no TEE must reject user login.
The 2026-09-05 x86 verification was blocked by the kernel’s unconditional
AArch64 register code, missing x86 C standard-library headers in Wasmtime’s
build, and the absent saved x86 firmware bundle. The product-split verification
above records the refreshed firmware cache and the current build blocker. These are test surfaces;
pass/fail results depend on the actual run and are reported with implementation
verification rather than inferred from target existence.

The debug CLI/TTY verification on 2026-09-05 passed the full AArch64 shell
scenario (including a UID 1000 terminal through transplant and rollback), the
separate authentication-only run, and the AArch64 app-registry, trace, and
user-storage-isolation regressions. The broader
`//testing/e2e/qemu/update:debugd_update_e2e_test_aarch64` failed during its
BexFS prerequisite with a migration preparation timeout, before reaching the
debugd transplant. Its prerequisite chain is retained unchanged.
`//testing/e2e/qemu/update:debugd_state_update_e2e_test_aarch64` separately
exercises debugd's retained partial-upload state, with app and TEE prelude state,
without requiring the storage-driver transplant chain; this focused migration
scenario passed. The final post-format run passed all eleven focused host test targets for this work
(including appd, usersd, and kernel core) after the CLI password-input
argument-group correction. `bazel run @rules_rust//:rustfmt` completed successfully.

## Graphical boot UI

The workstation graphics bundles package the D1 VirtIO-GPU driver, `splashd`,
and a minimal CPU `scened`, with replacement archives and generated FIDL.
On 2026-09-06, live replacement of all three components passed on AArch64 and
x86_64, with captured spinner/progress changes, identical takeover pixels,
splash exit, and post-transplant presentation. Authenticated AArch64 normal
takeover, normal x86_64 graphical takeover, and the graphical image booting
without a GPU also passed. Refreshed
nongraphical smoke boots passed on authenticated AArch64 and development x86_64.

The focused 16-target host batch, five x86 host configurations, shared userspace
and migration tests, product validation, and `//:heart_transplant_coverage_test`
passed. Deadline exhaustion and idle-wait bugs exposed by the graphical tests
were corrected and covered by the final runs. Required Bazel Rust formatting
completed. Frame timing and binary size exceed the design targets; firmware
framebuffer and pending-handoff rejection coverage is host-side. See
[boot UI](bootui.md) for exact commands, measurements, and current limitations.


RFC 0064 now adds the centralized pkgd source implementation, including a
configured font miss path, asynchronous app installation, OCI/TUF verification
and a protected package-state endpoint. Default products contain no remote
registry roots or mappings. The complete guest/lifecycle acceptance remains
outstanding; see [package resolution current state](rfcs/0064/CURRENT.md).
On 2026-09-13, 16 affected host targets passed, including retained database
ownership across close/reopen. The ARM guest passed malformed-resource and
cancellation probes and provisioned sealed credentials, then failed artifact
resolution with `UNAVAILABLE`. Subsequent incremental TUF lookup changes passed
15 TUF tests and 21 pkgd tests, including interrupted CAS writes and credential
generation isolation. Shared userspace, kernel routing, appd and RTC-only timed
checks also passed. The guest now validates RTC time and reaches authenticated
OCI metadata and payload URLs after directory handoff and stream fixes. The
last naturally completed ARM run failed with a TCP setup timeout before consumer
success. Later diagnostic runs were deliberately stopped; one showed an HTTP
deadline expiring after 294,908 of 361,234 application bytes. Configurable TCP/TLS
and HTTP limits subsequently passed focused host checks: 21 pkgd tests, four
lifecycle tests, four HTTPS/deadline tests, five shared network tests and the
client dependency-policy check. No confirmed guest result exists for these new
limits, and no verified guest artifact delivery is claimed.
Netstack's 18 public tests, three internal tests and five shared network tests
passed. Full guest delivery, replacement and reboot acceptance remain pending.
The lifecycle fixture also needs explicit ordering to prove reads remain pending
through service and credential replacement. Synchronous service/storage calls,
ignored TCP buffer sizing and TLS root-bundle handle cleanup are open source
findings. The authoritative [gap table](rfcs/0064/CURRENT.md#current-gaps)
separates implementation defects, missing assertions, unpassed guest scenarios
and final-tree architecture/product checks. No physical-hardware validation is
claimed for RFC 0064.
