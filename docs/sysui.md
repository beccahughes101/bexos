# System and User UI

SysUI and UserUI require the public resolve and fallback methods from
`bexos.fonts.FontProvider`. Their native Dioxus host resolves CSS family stacks,
uses Inter and JetBrains Mono as the local baseline, and selects Noto Arabic or
Devanagari fallbacks before shaping with Parley. The mapped font cache is rebuilt
from retained documents after heart transplant; neither shell can install fonts.

The graphical product bundles `bexos.app.sysui` and `bexos.app.userui`. Both are
Dioxus WASM services using the native renderer, shared
`com.bexos.lib.dioxus` package (version 0.2), and first-party `//lib/ui`
component prelude. Appd starts the selected
SysUI as UID 0. Usersd verifies passwords and unlocks the user's existing vault;
only then does appd resolve and launch that user's desktop. Nongui products do
not include or start either shell.

## Selecting packages

The configuration belongs to the stable package `bexos.platform.appd`.
All values use the existing typed configuration/preferences CLI; there is no
second shell-specific settings database or password store.
`config get` reads the operator overlay and its generation. To inspect resolved
system values, including bundled defaults, use
`bexctl prefs get bexos.platform.appd --uid 0`.

| Field | Scope | Default | Takes effect |
| --- | --- | --- | --- |
| `sysui_package` | System only | `bexos.app.sysui` | Reboot |
| `userui_package` | System default and per-UID override | `bexos.app.userui` | Logout and fresh login |

Read the generation before writing. These examples assume the preceding read
reported generation `0`; use the actual generation shown on your system.

```sh
bexctl config get bexos.platform.appd
bexctl config set bexos.platform.appd 0 sysui_package=string:com.example.sysui
bexctl config get bexos.platform.appd
bexctl config set bexos.platform.appd 1 userui_package=string:com.example.desktop

bexctl users unlock 1000 --password testpass
bexctl prefs get bexos.platform.appd --uid 1000
bexctl prefs set bexos.platform.appd 0 --uid 1000 userui_package=string:com.example.personal_desktop
```

Prefsd persists generations, schema validation, operator locks, system defaults,
and separate encrypted user preferences. A user cannot override `sysui_package`.
An operator lock on `userui_package` overrides user selection. Changing the
user selector while locked requires unlocking that user's preferences first.
Unlocking through the CLI alone does not start a graphical session.

An invalid, unavailable, ambiguous, or non-migratable selection generates a
serial/UI diagnostic and tries the bundled default. The saved preference remains
intact. If the bundled package also cannot start, user content remains hidden and
the existing CLI remains the recovery interface. Reset a selector with the normal
configuration/preferences commands and reboot or log out as appropriate.

## Login and desktop

Provision accounts through the existing CLI. The initial screen shows guidance
when there are no enabled accounts:

```sh
bexctl users create --uid 1000 --name alice --display-name Alice --password testpass
```

Click the user button or press Tab to cycle users. Password input is masked;
Enter or Sign In submits it to usersd. Failure clears the password and displays
an error. Passwords are overwritten after submission and excluded from shell
checkpoints and logs.

The desktop provides a wallpaper, Apps menu, taskbar, and floating app windows.
Scroll the Apps menu to reach further entries.
The launcher enumerates installed packages consuming Flatland and excluding
shell packages; launches use the desktop's UID-bound Opener. Title bars move
windows, the lower-right corner resizes them, clicking or using the taskbar
changes focus and stacking, and the title-bar close control stops the app's
processes in this session. Lock hides the complete user hierarchy and revokes
its input before usersd locks the account. Unlock resumes that session. Log Out
terminates its user processes and returns to the account picker.
The lock screen also provides Log Out, including when the desktop is unavailable.

## Package and IPC contract

A selectable package declares exactly one process with the matching role:

```prototxt
processes {
  name: "userui"
  runner: "wasm"
  shell_role: SHELL_USER
  service: true
  lifecycle { update_strategy: HEART_TRANSPLANT }
  # runner_options, limits and component imports follow the normal WASM contract.
}
```

Use `SHELL_SYSTEM` for SysUI. Ordinary launch paths reject shell entrypoints;
appd supplies role grants only on its internal selected-shell launch path.
Replacement manifests must retain the role and migratable entrypoint.
See the default manifests for the complete service/method declarations.

`bexos.shell.session` uses 64-bit UIDs. The selected process binds
SessionManager through ServiceDirectory ordinal 3. Login and display-reference
transfer require SysUI authority. Both shells can request lock/logout; app close
and UserShell callback registration are scoped to the active user's desktop.
UserShell receives primary-display attachment and session-state notifications.
Appd checkpoints the selected packages, UID, epoch, client and callback channels.
Compositor grants include the user-session generation. A retained UserUI grant
from an earlier login cannot attach, focus, or expose a later session; compositor
checkpoints preserve this generation along with view ownership.
Shell authentication and the CLI use separate usersd endpoints. The shell's
ordered connection checkpoints outstanding replies, so a timeout cannot turn
an earlier account-list response into a later authentication result.

Flatland ordinals 25–26 enumerate authorized child view references and focus
those children. References are channel handles created by scened and routed using
authenticated broker bindings. SysUI can embed only the selected user's shell;
UserUI can embed only ordinary apps belonging to its UID. Unattached apps are
invisible. Login/lock discards queued user events and cancels user captures;
hidden or unattached views cannot read input. UserUI never receives SysUI grants.

Native guest APIs cover child embedding, transforms, clipping, stacking,
viewport dimensions, and node scene batches. Existing application APIs remain
available. The WASM runner checkpoints logical native view state and transferable
resources, reconstructing rendering resources after adoption. UserUI checkpoints
window geometry/order and retained view references, and drops active drags.
Read-only session queries remain available during replacement so an outstanding
guest request can complete before checkpointing. The runner snapshots between
dispatches, retains the previous bulk snapshot if a refresh fails, and defers
cutover until the current dispatch and rendering work are complete.
SysUI and UserUI submit retained Dioxus documents for their own shell chrome and
controls. The runner stores those document bytes in UI migration state and
re-renders them when `prefsd` reports a new `bexos.ui.theme` generation, so
theme, text scale, reduce-motion, and high-contrast updates do not require shell
restart. Embedded application windows remain separate child views managed through
the existing Flatland reference APIs.
The authenticated WASM runner embeds build-time Pulley artifacts for the exact
composed default SysUI, UserUI, and Dioxus demo components, in addition to Brush.
An exact SHA-256 digest of the raw composed component selects each artifact;
package and checkpoint bytes cannot provide serialized executable code. Updated
or third-party shell components use the normal validated on-device compiler, and
standalone core WASM modules keep their existing instantiation path.
The runner reserves linear-memory address ranges up to the configured limit,
adding backing VMOs in 2 MiB chunks as the guest grows. Unused capacity does not
allocate kernel page bookkeeping. Growth retains the base address, and a failed
mapping leaves the previous WASM length intact.
Scened checkpoints the authenticated composition hierarchy. The kernel recycles
closed capability, channel and VMO storage; each new channel receives a fresh
endpoint identity. Kernel snapshot version 19 and incremental channel records
preserve those identities, including imports from earlier snapshots. SysUI loss
hides user content immediately and recovery requires authentication; UserUI loss
hides that subtree while the selected shell is recovered. The existing migration
coordinator retains the source on failed candidate adoption.
If UserUI exits while locked, appd retains its selected package and waits for
authentication before recreating it under the unlocked UID.

## Known gaps

The basic milestone deliberately stops at one display and one active graphical
user session. It does not yet implement graphical settings, multiple monitors,
fast user switching, widgets, system overlays, biometric prompts, or tiling.
Those remain future work in [the full design](rfcs/0060/README.md). Account setup
also remains a CLI operation; the no-user screen provides the command needed to
provision the first account instead of running a graphical setup wizard.

The current compositor admits 16 live views, including SysUI and UserUI, and the
launcher returns at most 64 entries. Closing a window stops that package's
processes in the active session, so a package cannot yet expose independently
closable top-level windows. Window content scales with the viewport rather than
receiving a richer display-density contract. The shells use a small ASCII bitmap
font and therefore do not yet provide internationalized typography or input.

The kernel has 48 process records for an entire boot. Records currently remain
allocated after a process exits, so repeated launches, logins, and shell
replacements can exhaust the table. Recovery requires a reboot. The acceptance
fixture splits window interaction and recovery/selection scenarios across three
boots of one persistent disk to stay within that limit.

Shell package selection and theming are intentionally declarative. There is no
graphical package picker, compatibility probe, package installation flow, or
theme editor yet. An operator or user installs a compatible package and writes
its package ID with the CLI; theme fields live in the typed
`bexos.ui.theme` preference package. Fallback reports an invalid selection and
preserves it for diagnosis; it does not repair or disable that preference
automatically.

The AArch64 end-to-end path is fully recorded below. The same complete x86_64
acceptance target is still a validation gap at this revision; the focused x86_64
migration path passes, including secure boot, live shell replacement, rollback,
and appd replacement. The latest full run reached the secure login screen and
started its unattached application probe, then exposed a 360-second debugd launch
envelope that was shorter than first-time on-device compilation under TCG. That
envelope is now 900 seconds and the matching host suites pass, but a complete
rerun has not been recorded after the correction. This is a missing validation
result rather than a separate x86 implementation.

## Validation

Standalone graphics fixtures explicitly set `//services/scened:legacy_standalone`;
production defaults to secure shell composition. Existing graphics wrappers set
that fixture flag through the architecture test transition.

Recorded validation for this implementation:

- Host tests pass for appd and shell authorization, scened composition and input,
  configuration and preferences, shell helpers, the WASM runtime and runner,
  graphics protocols, kernel allocation and migration, and UserUI window state.
  This includes resource churn, full and incremental snapshots, dispatch cutover,
  failed-refresh retention, selection precedence, invalid-package recovery,
  cross-UID rejection, and geometry/order/reference adoption.
- `//testing/e2e/qemu/sysui:sysui_e2e_test_aarch64` passes in 2,612.8 seconds.
  Across three boots it covers failed and successful login, two floating windows,
  drag, resize, focus, close, lock/unlock, logout, credential-input isolation,
  application subtree isolation, per-user selection isolation, invalid-selection
  fallback without preference mutation, reboot-applied system selection, shell
  loss, SysUI/UserUI/scened/appd transplant, and rejected-candidate rollback.
- `//testing/e2e/qemu/sysui:sysui_migration_e2e_test_x86_64` passes in
  3,886.5 seconds. It boots through the authenticated EFI and Trusty path, performs
  two live SysUI replacements, retains the source after a rejected candidate,
  replaces appd, and verifies that preference and frame service resume afterward.
- Focused AArch64 recovery, migration, and preference targets pass. Existing
  AArch64 Brush, WASM, app-registry, and graphics boot/replacement regressions also
  pass. The validation commands below rebuild their inputs through Bazel; no
  generated bindings or locally cached firmware images are source inputs.

Run the graphical acceptance targets with Bazel:

```sh
bazel test -c opt --test_tag_filters= --test_output=summary --test_timeout=14400 \
  //testing/e2e/qemu/sysui:sysui_e2e_test_aarch64 \
  //testing/e2e/qemu/sysui:sysui_e2e_test_x86_64
```

The fixture provisions its own accounts and preserves its disk across the three
boot phases. It drives the virtual keyboard and pointer and checks rendered
pixels, user state, process lifecycle, persisted selections, and replacements.
Cold component startup has a separate deadline from ordinary window operations.
The x86 targets consume the authenticated EFI loader, enrolled variable store,
kernel, and workstation vbmeta directly from their Bazel build graph, so the
run cannot silently reuse an older locally saved firmware image.

## Locale changes

Source integration is present; the local implementation and acceptance plan
remain incomplete. See [RFC 0066 current gaps](rfcs/0066/CURRENT.md#current-gaps-in-the-approved-local-scope).

The shells embed English Fluent catalogs compiled by Bazel. Each polls a shared
locale context before rendering; a new committed generation dirties the retained
document without recreating the password/input model, window list, focus state,
scroll offsets, or view IDs. Root documents carry display language and direction.
Subscriptions live in the native runner's migration checkpoint; shell restore
rebuilds its application context. Production catalogs currently fall back to
English for other languages. Separate multilingual fixtures are intended to
exercise translation switching; live switching and retained-state acceptance
have not passed. Existing credential-clearing behavior during shell migration
is separate from locale redraw behavior. See [localization](localization.md) for preferences and
[RFC 0066 status](rfcs/0066/CURRENT.md) for validation results.
