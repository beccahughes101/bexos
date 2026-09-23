# bexctl CLI reference

`bexctl` is the host client for the developer debug service. Build and run it with
`bazel run //tools/bexctl:bexctl -- ARGS`. Target commands require the QEMU serial
Unix socket: `bexctl --socket /tmp/debugd.sock COMMAND`.

## Help, output, and errors

`bexctl --help`, `bexctl help COMMAND`, and `bexctl COMMAND --help` work offline,
including nested commands; `-h` aliases `--help`. `bexctl --version` (or `-V`) prints the host CLI version;
`bexctl exec debugd.version` prints the target protocol version.

`bexctl completions bash|elvish|fish|powershell|zsh` emits a completion script on
stdout without connecting. Source or install the script using your shell's
normal completion setup. For example:

```sh
# Bash (current session)
source <(bexctl completions bash)
# Zsh: place this directory in fpath before running compinit
mkdir -p ~/.zsh/completions
bexctl completions zsh > ~/.zsh/completions/_bexctl
# Fish
mkdir -p ~/.config/fish/completions
bexctl completions fish > ~/.config/fish/completions/bexctl.fish
```

For Zsh, add `fpath=(~/.zsh/completions $fpath)` and
`autoload -Uz compinit && compinit` to `.zshrc`. Elvish and PowerShell users can
save the corresponding generated script and load it from their shell profile.

Put `--format table|json|tsv` **before the command**. The default table has column
headings and an explicit empty-result message. JSON outputs arrays of objects,
with numeric and boolean values preserved. TSV emits data rows without headings.
Diagnostic-backed app progress and update/kernel status commands return a `diagnostic` and a
`result` text field in each output mode. Control characters are escaped in human-readable cells. Binary exports and
`exec`/shell streams retain their bytes. Diagnostics go to stderr.

Argument errors exit 2; connection, protocol, and local failures exit 1. Successful
commands exit 0. Remote `exec` and shell exit codes from 1 through 255 are
propagated; other failures map to 1. No command connects merely to display help.

`BEXOS_DEBUG_CALL_TIMEOUT_SECONDS` overrides the RPC deadline (valid range
1–86400). Defaults are 120 seconds for normal RPCs and 600 seconds for shell
opening, bundle installation commit, and app launch, which can include a
300-second nested VFS operation. Missing, malformed, or out-of-range values
use the operation’s default.
The current transport is BXD1 over the QEMU serial socket, with a 64 KiB frame
payload limit. Process snapshots exceeding that limit return an explicit error
instead of truncating records. HTTP/2, network transports, and log streaming remain future work.

## Inspection and applications

- `bexctl health`: service name, status, and target version.
- `bexctl ps [--wide]`: PID, state, process, package, and resource-group name/ID.
  Wide output includes the main thread and parent group. JSON always contains
  these details. Unknown group metadata is identified explicitly; old targets
  without the new fields remain readable.
  TSV always emits PID, state, process, package, group ID, group name, main-thread
  ID, parent ID, and parent name, in that order. JSON uses `null` for missing
  metadata; human-readable and TSV fields use an em dash.
- `bexctl apps`: installed packages, names, state, source, and protection status.
- `bexctl install BUNDLE`: upload and install a local application bundle.
- `bexctl install-url URL`: install from an HTTPS URL.
- `bexctl reload-well-known DOMAIN`: refresh domain association metadata; supply
  a domain rather than a URL or path.
- `bexctl uninstall PACKAGE`: uninstall a mutable package.
- `bexctl launch PACKAGE PROCESS [ARG0] [--uid UID]`: launch a process. ARG0 and
  UID default to 0; user launches follow appd's vault and permission checks.
- `bexctl app`: application diagnostics.
- `bexctl app progress [PACKAGE]`: completed/error counters; PACKAGE defaults
  to `bexos.platform.storage_verify`.
- `bexctl test-app`: development manifest/executable upload operations.
- `bexctl test-app install PACKAGE MANIFEST ARTIFACT`: upload both test-app
  streams and commit them. Supply the target's compiled manifest representation;
  source manifests are prototxt and must be compiled through Bazel.
- `bexctl test-app launch PACKAGE PROCESS [ARG0]`: launch an uploaded test app;
  ARG0 defaults to 0.

Example: `bexctl --socket /tmp/debugd.sock --format json ps`.

## Component configuration

`bexctl config` reads or changes a component's typed configuration.

- `bexctl config get PACKAGE [-o PATH]`: show generation and byte count, optionally
  save the compiled configuration. Long output flags: `--out`, `--output`.
- `bexctl config set PACKAGE EXPECTED_GENERATION NAME=TYPE:VALUE...`: set fields;
  at least one assignment is required. Types: `bool`, `u32`, `u64`, `string`,
  and `bytes`. Booleans accept true/false/1/0; bytes are even-length hex, optionally
  prefixed with `hex`. Names contain ASCII letters, digits, and underscores.
- `bexctl config reset PACKAGE EXPECTED_GENERATION`: restore manifest defaults.

Generation mismatches are errors, preventing an unnoticed overwrite. Configuration
sources and app manifests remain prototxt; downloaded config blobs are artifacts.

## Users

`bexctl users` exposes usersd-backed account and vault operations. UIDs are 64-bit.

- `bexctl users list`: list accounts and lock/disabled state.
- `bexctl users get UID`: inspect one account.
- `bexctl users create --uid UID --name NAME --password PASSWORD [--display-name NAME]`:
  create an account; display name defaults to the login name.
- `bexctl users update --uid UID --name NAME [--display-name NAME] [--disabled]
  [--current-password PASSWORD --new-password PASSWORD]`: update identity fields
  and optionally replace the password. Both password options must be supplied
  together. Display name defaults to NAME; disabled defaults to false.
- `bexctl users delete UID`: delete an account.
- `bexctl users unlock UID --password PASSWORD`: verify the password and unlock
  the user's vault, including password verification for already unlocked users.
- `bexctl users lock UID`: lock the user's vault.

## Interactive terminals

`bexctl shell` opens the preferred `bexos.shell.ShellProvider`. Exactly one identity
selector is required:

- `bexctl shell --system`: use system UID 0 and the system provider preference.
  The existing developer debug connection authorizes this mode; no user password
  is accepted.
- `bexctl shell --user NAME`: authenticate an exact login name.
- `bexctl shell --uid UID`: authenticate a nonzero UID.

User modes prompt on `/dev/tty` with echo disabled. `--password PASSWORD` supplies
an explicit password; `--password-stdin` reads one line before subsequent stdin
bytes become terminal input. A trailing LF or CRLF is removed from that line.
These options conflict. Login names contain 1–64 UTF-8 bytes without control
characters; shell passwords contain at most 256 UTF-8 bytes. Debugd verifies the password
through usersd before launching the provider. Disabled users and UID 0 cannot use
the user-login route. Closing a terminal does not automatically lock the vault.
The initial environment supplies `USER`, `HOME=/data`, `PATH=/pkg/bin:/system/bin`, and
`TERM=xterm-256color`. `/data` is the provider’s existing identity-scoped app data
mount; the storage service’s internal user-home path is not a namespace path.

The host enters raw mode after opening succeeds, forwards window changes, and
restores terminal settings on exit. The fallback window size is 24 rows by 80
columns. In an interactive host terminal, Ctrl-] closes the session. Ctrl-C/Ctrl-Z bytes
are interpreted according to the provider's terminal mode. Redirected input/output
uses raw bytes without terminal echo. External SIGINT is forwarded as Interrupt;
SIGTERM, SIGHUP, SIGQUIT, and SIGTSTP forward a control signal before closing with
128 plus the host signal number. Input EOF closes stdin while output drains;
the provider's exit status is returned after final stdout/stderr delivery.

There is one active session per debug transport. Valid debug transport activity renews a 30-second lease;
a disconnected frontend is cleaned up when the lease expires. Active terminal
handles, identity, and EOF state participate in debugd heart transplant. Migration
quiescence pauses lease expiry. Credential-bearing partial requests are excluded
from migration snapshots rather than being copied to a replacement.

The image configuration now includes `bexos.app.brush_shell`, built by
`bazel build //apps/brush_shell`. Appd selects its `terminal` process as the
configured fallback when the identity has no explicit shell preference. It is
storage-installed and launches on demand. Fresh-image behavior has not been
reverified in QEMU for this change.

Brush's current transplant support covers idle shell state and partial terminal
input. Transplant is deferred while the evaluator or jobs are active. Basic pipelines, substitutions, and background execution with `wait` are tested.
Complete job control and active execution continuation remain unfinished;
see [Brush implementation status](brush-shell.md) and
[the terminal design](rfcs/0033/README.md).

## Tracing

`bexctl trace` controls the target trace session.

- `bexctl trace start [OPTIONS]`: start recording.
- `bexctl trace status`: inspect state, categories, buffers, format, producer/event
  counts, and dropped events.
- `bexctl trace stop -o PATH`: stop and export the recorded bytes.
- `bexctl trace record -o PATH [--duration-ms MS] [OPTIONS]`: start, wait, stop,
  and export. Duration defaults to 5000 ms.

Start/record options: `--categories LIST` (default `all`, comma-separated category
names), `--buffer-size-kb N` (positive; default 2048), `--buffer-mode circular|oneshot`
(default circular), and `--format perfetto|fxt` (default perfetto). `pftrace` aliases
perfetto; `legacy` and `legacy-bexos-fxt` alias fxt. Here `--format` selects the trace
file format, independently of root structured output. `--output` aliases `-o`.
See [Tracing](tracing.md) for categories and capture interpretation.

## Updates

`bexctl update` uploads, stages, applies, and inspects updates.

- `bexctl update check [PACKAGE|TEE|KERNEL|HYPERVISOR] [--all] [--stage] [--apply] [--activation live|on-reboot]`: query the
  configured feed. `--all` conflicts with a target; `--apply` implies staging.
  Lowercase firmware selectors are accepted. `--activation` requires `--apply`
  and a single `TEE` or `HYPERVISOR` selector. Trusty accepts reboot activation
  only. Without an explicit selector, use the update service's default
  all-selector behavior.
- `bexctl update app MANIFEST ARTIFACT`: upload and apply a signed app update.
- `bexctl update service MANIFEST ARTIFACT`: upload and transplant a service.
- `bexctl update platform MANIFEST ARTIFACT`: upload and apply a kernel/TEE update.
- `bexctl update firmware MANIFEST ARTIFACT --activation live|on-reboot`: upload signed x86 monitor or Trusty firmware with explicit activation. Trusty accepts `on-reboot` only. Pending reboot does not advance the committed firmware generation. Inspect the protected outcome with `bexctl tee update-status`.
- `bexctl update status`: inspect the current update operation.
- `bexctl update apply app|service|platform`: apply an already staged update.
- `bexctl update stored-service TARGET GENERATION [ARCHIVE_ID]`: transplant from
  the archive store; archive ID defaults to TARGET.
- `bexctl update service-status [TARGET]`: inspect migration; target defaults to
  `bexos.platform.appd`.
- `bexctl kernel`: kernel diagnostics.
- `bexctl kernel update-status`: inspect platform update state.

## TEE

`bexctl tee` exposes TEE management and diagnostics.

- `bexctl tee info`: presence, implementation kind, OS version, rollback floor.
- `bexctl tee apps`: all trusted applications, UUIDs, package identities, versions,
  session counts, storage usage, and protection status.
- `bexctl tee list-packages`: only package-managed trusted applications.
- `bexctl tee install-app FILE`: install a trusted app and return its UUID.
- `bexctl tee uninstall-app UUID`: uninstall a trusted app.
- `bexctl tee open-session UUID`: return a session ID.
- `bexctl tee close-session SESSION_ID`: close a session.
- `bexctl tee invoke SESSION_ID COMMAND_ID [PAYLOAD]`: invoke with optional file
  bytes; stdout contains the raw response.
- `bexctl tee update-core MANIFEST IMAGE`: upload a signed platform update and
  apply the TEE image.
- `bexctl tee update-core-raw GENERATION TARGET IMAGE`: submit the low-level TEE
  RPC with a BLAKE3 image hash; the image and request must fit one 64 KiB frame.
- `bexctl tee update-status`: inspect TEE update status.
- `bexctl tee diagnostic keymint|orchestrator|authmgr|concurrent`: run the selected
  target diagnostic. Diagnostics may create temporary TEE resources.

UUIDs contain 32 hex digits, with optional hyphens. Session IDs are u64; command
IDs are u32. Unsupported target operations return errors rather than fabricated
results.

## Diagnostic exec reference

`bexctl exec COMPONENT [-- ARG...]` invokes the target's allowlist, not arbitrary
shell commands. stdout/stderr are forwarded, and remote exit status is preserved.

| Component | Arguments / behavior |
| --- | --- |
| `debugd.version` | Target version |
| `kernel.ps` | Kernel process listing |
| `app.progress` | Optional PACKAGE; default storage_verify |
| `update.status` | Update status |
| `update.apply_app` | Apply staged app |
| `update.apply_service` | Apply staged service |
| `update.apply_platform` | Apply staged platform artifact |
| `update.apply_stored_service` | TARGET GENERATION [ARCHIVE_ID] |
| `update.service.status` | Optional TARGET; default appd |
| `kernel.update.status` | Kernel platform update status |
| `tee.info` | TEE summary |
| `tee.apps` | Trusted-app list |
| `tee.keymint_smoke` | KeyMint diagnostic |
| `tee.orchestrator_smoke` | Orchestrator diagnostic |
| `tee.authmgr_smoke` | AuthMgr diagnostic |
| `tee.concurrent_smoke` | Concurrent storage diagnostic |
| `tee.update.status` | TEE update status |

Unrecognized components return remote exit 127. Prefer the corresponding typed
CLI command for argument validation, help, and structured output.

## User preferences and configuration policy

- `bexctl prefs get PACKAGE --uid UID [-o PATH]`: inspect effective typed values,
  generation, and locks; optionally save the BEXCFG snapshot.
- `bexctl prefs set PACKAGE EXPECTED_GENERATION --uid UID NAME=TYPE:VALUE...`:
  update user-editable values using the existing scalar assignment syntax.
- `bexctl prefs reset PACKAGE EXPECTED_GENERATION --uid UID`: remove the user
  layer while retaining assembly defaults and operator policy.
- `bexctl config lock PACKAGE EXPECTED_GENERATION NAME...`: explicitly lock
  MDM-lockable fields.
- `bexctl config unlock PACKAGE EXPECTED_GENERATION NAME...`: remove runtime
  locks; immutable product locks cannot be removed this way.
- `bexctl config policy PACKAGE`: inspect the operator generation and lock set.

UID selection is explicit. Nonzero users must be unlocked. Receiver-enabled
apps prepare changes before they are saved; rejection or timeout leaves the
previous generation unchanged. Older apps receive changes on their next launch.
A committed change with outstanding receiver acknowledgements is reported as
`CommittedPending`; read the current generation before retrying a request whose
transport result is uncertain.

See [preference validation](services.md#preference-validation) for the host and
QEMU coverage of these commands and their service transport.

## Locale preferences

Source integration is present; the local implementation and acceptance plan
remain incomplete. See [RFC 0066 current gaps](rfcs/0066/CURRENT.md#current-gaps-in-the-approved-local-scope).

Locale settings use the existing preference commands and the package
`bexos.locale.preferences`; there is no separate settings panel. For example,
after reading the current generation:

```sh
bexctl prefs get bexos.locale.preferences --uid 1000
bexctl prefs set bexos.locale.preferences 0 --uid 1000 \
  language_priority=string:de-DE,en-US override_region=string:en-US
```

Replace `0` with the reported generation. The comma-separated language list
accepts up to eight distinct canonical tags. English-only production catalogs
fall back to English; regional data coverage is build-configured. See
[localization](localization.md) for all fields and defaults.
