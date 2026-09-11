# Brush shell implementation status

`//apps/brush_shell` packages upstream Brush revision
`a2620d71bf3d08ca4792655836f9f916c7bb73ff` as a signed WASI 0.2 service component.
Bazel owns compilation, WIT/FIDL/protobuf generation, and archive assembly. Local
upstream adaptations are explicit patches under `third_party/patches`.

The package ID is `bexos.app.brush_shell`. The `terminal` process declares heart
transplant, provides `bexos.shell.ShellProvider`, and launches on demand. Portable
base assembly, storage preinstall mappings, and both architecture system-image
manifests include it. Appd's prototxt fallback selects Brush when an identity has
no explicit preference. Existing persisted user preferences take precedence.
Initial CWD and HOME are `/data`; the default PATH is `/pkg/bin:/system/bin`.

## Debug shell startup

The normal QEMU developer `:run` targets select optimized boot artifacts even
when invoked without `-c opt`, matching the guest configuration used by the QEMU
regressions. The host launcher remains independently configured.

On macOS the launcher loads a Bazel-built activity shim into its QEMU child.
The shim retains an `NSProcessInfo` user-initiated activity for that process,
allowing idle system sleep. This addresses disk and timer throttling while Cocoa
is covered or the Mac is locked. Display sleep, screen locking, and global macOS
preferences are unchanged; no auxiliary process remains after VM teardown.
The native-display reproduction stalled on filesystem work while the equivalent
headless workstation passed. See the upstream
[QEMU App Nap report](https://gitlab.com/qemu-project/qemu/-/issues/334).

The QEMU developer launcher waits for debugd's appd and usersd connections and
appd's lifecycle dispatch loop before announcing readiness. This includes the
final bootstrap disk sync. The early socket-ready marker alone does not mean a
shell provider can be resolved. Developer boot allows up to 600 seconds by default;
`BEXOS_QEMU_TIMEOUT_SECONDS` still overrides that boot deadline.
The default workstation also builds `scened` and its transplant replacement with
the optimized-artifact rule: its 88 MiB fastbuild executable exceeded the kernel's
64 MiB VMO limit and aborted appd before shell dependencies became available.

Cold Brush compilation previously exceeded the existing 360-second provider
lookup deadline. The shared heap now uses an intrusive AVL tree with subtree
capacity summaries, avoiding linear scans across fragmented free extents while
preserving alignment, coalescing, and in-place resizing. The WASM runtime disables
Cranelift optimization passes to favor on-device compilation latency. This trades
interpreter bytecode optimization for faster cold startup; WASI components still
use their optimized Bazel build.

Bazel now precompiles Brush to Pulley using the shared engine configuration and
embeds a compressed artifact and its raw WASM digest in the runtime executable.
Decoding has an exact, bounded output length. The installed runtime and its
replacement omit the static symbol table to reduce ELF loading work. Only that
exact input selects the trusted artifact; updated components continue through
normal compilation. Raw input size admission, instance resource limits, and
migration validation still apply. No serialized code from packages or checkpoints
is accepted, and no generated artifacts are committed. The shell provider timeout
has not been increased.
The WASI platform explicitly selects the Rust linker so an app archive and a
runtime dependency resolve to identical component bytes across Bazel transitions.

Deadline scheduling now spends graphics-task budgets at every execution-accounting
boundary, including voluntary yields, rather than only at timer interrupts.
Two yielding graphics tasks can no longer retain their budgets indefinitely and
starve the filesystem and shell services. Timer handling does not charge that
execution twice. Existing scheduler snapshots retain the remaining budgets.

Package-directory RPCs use the same 300-second durability window as other VFS
requests. The previous 180-second override could expire before a nested filesystem
commit and leave an untagged reply queued for a subsequent lookup.

The native WASM service loop prepares a checkpoint when a migration request is
pending at a safe point, rather than serializing the entire component and shell
after every idle dispatch. Exact record changes drive migration catch-up, busy
dispatches invalidate stale checkpoints, and inactive sources release saved
snapshots. Each top-level service dispatch receives its manifest fuel allowance;
a single runaway dispatch still traps, while idle polling no longer exhausts a
lifetime budget. Commands and child admission retain their existing fuel limits.

## Implemented paths

- The existing `bexctl`/debugd frontend uses shared TTY control and separate stdin,
  stdout, and stderr sockets. Native and WASM transports preserve the original
  PTY ordinals and one-time stream transfer.
- Brush supplies parsing, variables, aliases, functions, conditionals, loops,
  expansions, sourced scripts, redirection, and builtins. Cooperative tasks and
  bounded in-process pipes support basic pipelines, command substitution, and
  background execution with `wait`. `echo` retains unwritten output across yields.
- Manifest command declarations select declared non-service processes. Appd
  resolves the active installed catalogue and launches commands with verified
  runner paths, bound identity, explicit CWD capability, arguments, environment,
  and stdio. `/pkg/bin` and `/system/bin` candidates follow PATH order;
  `package:command` disambiguates providers. Failures use 126/127 statuses.
- Shared utilities package `ls`, `cat`, `grep`, `mkdir`, `ps`, and `kill`.
  Appd filters process listing and signalling by identity. Process-control
  endpoints support status, exit notification, signals, stopping, and resuming;
  appd includes bindings and pending completions in its migration state.
- Idle Brush checkpoints preserve interpreter state, terminal modes, size,
  pending bytes, EOF, and partial source input. Restore validates resource types,
  rights, and exclusive ownership before activation. Runtime initialization and
  external I/O occur after activation. Failed validation leaves live source state
  available for rollback.

## Remaining work

This implementation does **not** complete the full terminal design:

- Active evaluators, outstanding tasks/jobs, and non-terminal open files defer
  transplant. There is no serialized execution continuation for scripts, loops,
  substitutions, builtin progress, job waits, or pending command I/O. Running
  commands have not been proven to survive Brush replacement.
- Foreground/background signal ownership, Ctrl-Z/`fg`/`bg`, shell signal traps,
  and complete job control remain incomplete. Appd currently implements INT,
  TERM, and KILL through process termination rather than user signal handlers.
- Arbitrary executable files in other PATH directories are not launched.
  Readable shell scripts can be sourced with `.`; executable script invocation
  and complete filesystem/platform compatibility need further work.
- Backpressure is tested for `echo` and the external stream pump. Other builtins
  that use synchronous stream operations still need cooperative I/O adapters.
  External command terminal detection does not yet receive a session control grant.
- Non-system identity isolation, preference/reboot persistence, and live
  appd/debugd/Brush replacement acceptance are not reverified by the system-shell
  startup regression. No x86_64 guest acceptance is claimed for this correction.

## Verification

Startup correction verification:

- The final combined aarch64/E2E Bazel run passed all 20 targets: 18 executed and
  two reused passing test results (userspace and TTY).
- The exact developer commands were exercised with the native workstation display:
  `bazel run --config=aarch64 //device/virtual/qemu/workstation:run`, followed by
  `bazel run //device/virtual/qemu/workstation:debugd -- shell --system`.
  Boot reached readiness in 44.7 seconds and Brush opened in 28.9 seconds.
  The tunnel accepted input, returned stdout and stderr, remained responsive after
  30 seconds idle, and propagated exit status 7. The VM was stopped afterward.
- Sixteen focused host test targets passed: allocator, both libc suites, Brush's
  component/heap/PATH tests, CLI utilities, WASM runtime, WASM runner, and QEMU
  launcher, embedded-code selection and compatibility, userspace, TTY, appd,
  kernel core, and architecture.
  Runner coverage includes sustained dispatch and trapping a runaway dispatch.
  The allocator tests
  validate balance, alignment, preserved live bytes, resize, and coalescing across
  thousands of fragmented extents.
  The 137 kernel core tests include yielding deadline tasks, avoiding duplicate
  budget charges, and scheduler snapshot preservation; both architecture tests
  also passed.
- `//testing/e2e/qemu/bexfs:brush_shell_e2e_test_aarch64` passed in 40.6 seconds.
  The system shell opened in 15.5 seconds and verified the embedded-code path,
  Brush provider, UID 0, idle polling, prompt, separate stdout/stderr, and exit status 7.
- `//testing/e2e/qemu/bexfs:brush_workstation_shell_e2e_test_aarch64` passed in
  63.6 seconds, opening Brush in 27.1 seconds. It runs the same checks against
  the workstation image with graphics and input devices. Both shell regressions
  also verify that QEMU loads the native activity shim on macOS.
- `//testing/e2e/qemu/wasm:wasm_e2e_test_aarch64` passed in 172.0 seconds with
  `BEXOS_WASM_MIGRATION_ONLY=1`: rollback, service replacement, trusted-runner
  replacement, and preservation of the live client, WASI descriptor, stream
  offset, and pollable. The persistent client now lives until harness teardown
  rather than exiting during a slow runtime-image staging operation.
- `//:heart_transplant_coverage_test` passed in 1.1 seconds without a cached result.
- `bazel run @rules_rust//:rustfmt` and `git diff --check` completed successfully.

Recorded results for the initial Brush implementation (before the startup
correction above):

- Eleven non-QEMU test targets passed. The final Brush suite rerun also passed
  after formatting, including preservation of partial input, rejection of aliased
  checkpoint handles, large-output backpressure, and real WASI `cat`/`grep` runs.
- `//apps/brush_shell`, `//apps/brush_shell:replacement_archive`, and
  `//kernel:kernel_bin` built successfully from the final formatted sources.
- `bazel run @rules_rust//:rustfmt` and `git diff --check` completed successfully.
- The initial unoptimized component test exceeded its time budget. The public
  test target now uses an optimized host and passed on rerun.
- The combined architecture image build failed on missing x86_64 Trusty firmware.
  Neither a successful combined image build nor QEMU acceptance is claimed.


Use `bazel test //apps/brush_shell:tests` for real Brush component tests, PATH
candidate tests, and reusable utility tests. The component acceptance test uses
an optimized host to bound Pulley compilation time and exercises the actual
compiled guest. Appd protocol interactions use a test host; they are not evidence
of native appd process launch on a booted image.

The associated non-QEMU suite covers `//lib/tty:tty_tests`,
`//lib/wasm_runtime:wasm_runtime_tests`, `//lib/userspace:userspace_tests`,
`//lib/bexos_libc:bexos_libc_tests`, `//services/appd:appd_tests`,
`//services/wasm_runner:wasm_runner_tests`, `//kernel/core:core_tests`, and
`//:heart_transplant_coverage_test`. The archive targets are `//apps/brush_shell`
and `//apps/brush_shell:replacement_archive`. Format Rust with
`bazel run @rules_rust//:rustfmt`.

The full intended terminal ecosystem, including future GUI and SSH frontends,
remains in [the terminal design](rfcs/0033/README.md).
