# RFC 0066 current implementation

Status: implementation present in the working tree; **the approved local plan is
not complete or fully validated**. This is the handoff snapshot after the
implementation run was interrupted on 2026-09-13. The full long-term design
remains in [README.md](README.md); APIs and packaging are documented in
[localization.md](../../localization.md).

## Code present

- `build/rules/locale.bzl` compiles Fluent sources with pinned `fluent-syntax`
  0.12.0 into versioned, indexed `strings.bexres` catalogs. Runtime resolution
  does not parse FTL. Separate catalog, compiler, settings, formatting, platform
  mapping, and UI libraries implement references, attributes, interpolation,
  selectors, fallback, and bounded evaluation.
- Bazel exports `cldr.bexloc` using pinned ICU4X 2.2 tooling and checksummed
  CLDR 48.2 / ICU 78.1rc source archives. Formatting covers numbers, currencies,
  civil dates/times/calendars, lists, plurals, and direction. Currency's
  experimental dependency stays behind the formatting wrapper.
- `localed` has separate service, binding, storage, and migration modules. It
  loads one CLDR asset and distributes read-only VMO duplicates. Public requests
  use the authenticated binding UID; only appd may resolve another UID.
- The prototxt `bexos.locale.preferences` package uses prefsd's existing encrypted
  persistence and policy handling. Defaults are en-US, US customary measurements,
  H12, and Sunday. The comma-separated language list accepts at most eight
  distinct canonical tags; an empty regional override follows the first language.
  A private prefsd snapshot/watch feed admits only localed and suppresses
  prepared or uncertain transactions.
- Startup v10 carries optional locale state. Frozen v6–9 and existing v2–5
  decoders remain. Appd omits locale state for early services. Both architecture
  product definitions include localed, its preferences package, and CLDR assets.
- Native clients own read-only mappings. WASM formatting runs in the native
  runner; CLDR tables do not enter application linear memory. Application
  catalogs and translation resolution remain in the application.
- `LocaleContext`, `use_locale(context)`, and `t!(context, ...)` integrate with
  retained-document invalidation. SysUI, UserUI, and the demo use English
  catalogs. Language/direction are passed through style, layout, and shaping.
  Migration records retain locale resources and notification generations;
  mappings and contexts are reconstructed after adoption.

These describe source behavior, not a completed boot or live-migration
acceptance result.

## Verified checks

The last successful focused Bazel run completed **9 of 9 test targets**:

- `//apps/userui:tests`
- `//lib/i18n/formatting:tests`
- `//lib/ui/runtime:tests`
- `//lib/wasm_runtime:wasm_runtime_tests`
- `//services/appd:appd_tests`
- `//services/appd:shell_policy_tests`
- `//services/localed:tests`
- `//services/prefsd:prefsd_tests`
- `//services/wasm_runner:wasm_runner_tests`

Earlier passing runs covered `//tools/locale_bundle:tests`,
`//lib/i18n/settings:tests`, `//lib/ui/i18n:tests`,
`//lib/userspace:startup_compat_tests`, `//apps/sysui:tests`, and
`//lib/flatland_text:tests`. The startup fixture suite checks versions 6–10.
The focused run also built the native and WASM localization fixture archives.

The final broader attempt passed `//:heart_transplant_coverage_test` and reused
the passing startup compatibility result, but stopped on a compiler-test output
path collision. The source was moved from `tests/compiler.rs` to
`compiler_tests.rs` to address that collision; **the correction has not been
rerun**. That failed invocation is not a passing regression or product-closure run.

An earlier scoped `bazel run @rules_rust//:rustfmt` completed. The final formatting
pass was interrupted while waiting for Bazel, so later edits still need formatting.
Several bounds, handle-cleanup, lock handling, migration validation, RTL test,
and multilingual-fixture changes postdate the successful host runs.

## Current gaps in the approved local scope

1. **Lock versus deletion revocation needs completion.** Prefsd closes the private
   watch for both events. Localed discards cached settings and public listeners
   but retains public bindings to allow rewatch after unlock. The private feed
   does not identify deletion, and bindings contain a UID without an account
   incarnation. An old binding surviving deletion could therefore rewatch a
   newly created account using that UID. Distinguish deletion/revocation from
   temporary lock, revoke stale authority, and test the original live binding.
   The added UID-reuse fixture launches a new client; it does not prove stale
   bindings are revoked.
2. **Final host regression and formatting passes remain outstanding.** Recheck
   the compiler-test path correction, malformed-catalog bounds, real Russian /
   Arabic plural resolution, RTL placement, cleanup changes, and migration
   validation. Earlier green results do not certify the latest working tree.
3. **Neither localization QEMU acceptance target has passed or reached the
   localization scenario.** The first dual-architecture attempt stopped on
   missing saved firmware. ARM's `//third_party/trusty:refresh_image` subsequently
   succeeded. The x86 Trusty refresh was interrupted; the integrated EFI refresh
   still needs its saved Trusty prerequisite and must then be rerun.
4. **Live behavior remains unverified:** visible language/direction changes,
   retained counter/input/focus/scroll/window state, encrypted locale persistence
   across reboot, and account lock/unlock/deletion behavior. German/Russian/Arabic
   fixture code covers these scenarios but has not demonstrated them in QEMU.
5. **Heart transplant acceptance remains unverified** for localed, prefsd, appd,
   and the localized WASM fixture followed by another successful locale change.
   Host checkpoint tests and manifest coverage do not establish that result.
6. **Resource and race coverage is incomplete.** Read-only mapping and handle
   closure checks exist in the native fixture but have not run in a guest.
   Shared backing identity and failed-launch resource counts need explicit
   checks. Snapshot/watch ordering, notification backpressure, disconnects, and
   rejected transactions need guest-level race coverage beyond the current
   single-threaded logic and host tests.
7. **Native/WASM parity and architecture coverage need completion.** The fixture
   compares number formatting and plural results; date/calendar/hour-cycle,
   currency, and list parity still need coverage. The final two-architecture
   product/bootfs closure run did not complete. RTL host coverage has not been
   rerun after its latest test addition; live RTL pixels remain unverified.

## Shipped coverage and deliberately deferred design

Production SysUI, UserUI, and demo catalogs contain **English only** and fall back
to English for other language choices. Production formatting data defaults to
en-US; additional CLDR locales are a Bazel build setting. German, Russian, and
Arabic translations are test-only fixtures intended to demonstrate live switching,
not shipped production translations or a verified acceptance claim. Regional
formatting data absent from the blob falls back to en-US.

Online language packs and package-daemon integration remain pending RFC 0064.
There is no settings panel or supplementary-data reload implementation; the FIDL
reload event is reserved for future use. Named time zones, collation,
segmentation, arbitrary unit conversion, and future locale-pack policy remain
outside this agreed local slice. The long-term README retains those designs.

## Resume validation

After resolving the revocation gap and formatting the changed Rust targets,
rerun the focused suites above plus catalog/context/settings tests and
`//testing/build/architecture:{closure_test,bootfs_closure_test,workstation_bootfs_closure_test,emulated_bootfs_closure_test}`.
Refresh missing firmware through Bazel, then run:

```sh
bazel test --config=e2e \
  //testing/e2e/qemu/localization:localization_e2e_test_aarch64 \
  //testing/e2e/qemu/localization:localization_e2e_test_x86_64
```

The firmware build on this host required an explicit installed libclang path and
host-tool paths. Those environment prerequisites and the interrupted x86 refresh
must be resolved before interpreting any QEMU result. Keep generated firmware,
catalogs, CLDR blobs, and generated bindings out of source commits.
