# WebAssembly runtime

[SysUI and UserUI](sysui.md) exercise native UI resource migration. Composed
applications preserve their lifecycle exports, and native view IDs, assets, and
fences participate in runner checkpoints.

Appd launches raw WASM core modules and WASI 0.2 command components from BootFS
and disk packages through the trusted native runner. The shared runtime supports
checked host resources, signed and unsigned child sandboxes, and explicit
checkpoint migration for core and component services.

QEMU has verified boot/disk command launch, child execution, trap recovery,
failed-restore rollback, service updates with a live paused child, and replacement
of the trusted runner binary while a client remains connected. Host tests also
verify descriptor/stream adoption without reopening resources, network capability
checks through real WASI imports, signature tampering, resource limits, and
malformed state. The expanded QEMU suite also passes descriptor/stream migration, native network permission checks, and memory growth across the 64 MiB VMO boundary.

## Brush and explicit command streams

`//apps/brush_shell` is a WASI 0.2 service component, built through
`build/rules/wasm.bzl` with isolated Brush/Tokio dependencies. Its prototxt manifest
selects the WASM runner and heart transplant. The default target and
`:replacement_archive` produce signed `.bex` archives; `:wasm`, `:manifest`,
`:utilities_wasm`, `:path_tests`, and `:component_tests` are separately addressable.
Guest components and the compiler-heavy component test use optimized Bazel
configurations even when the surrounding build is fastbuild.

Checked component imports provide local socket/channel pairs, partial socket
reads and writes, readiness, half-close, and capability duplication. Admission
checks type, rights, resource budgets, and restore phase before host side effects.
Directory export delegates an existing WASI directory capability to appd without
reopening it by an ambient path. Resource metadata can be inspected during restore
without doing external I/O. WASI stdin uses an explicit `wasi:stdin` grant;
command stdout/stderr and CWD are explicitly delegated as well.

Brush checkpoints idle interpreter state and terminal buffers. Active evaluator
continuations and jobs currently defer transplant. This does not yet meet the
active-command migration design; see [Brush implementation status](brush-shell.md).

## Code and build layout

- `lib/wasm_abi`: bounded manifest options and runner startup prelude.
- `lib/wasm_engine`: shared Pulley compiler configuration for host build tools and
  the device runtime.
- `lib/wasm_runtime`: engine/platform integration, checked resources, aggregate
  budgets, core hostcalls, component bindings, WASI, children, and checkpoints.
- `services/wasm_runner`: native startup, BexOS filesystem/netstack/trust adapters,
  component composition, Dioxus UI hostcalls/rendering, service dispatch, and
  migration transport.
- `testing/wasm`: WAT fixtures compiled and packaged by Bazel.
- `testing/e2e/qemu/wasm`: product launch and trap tests.

Wasmtime is pinned to 48.0.1. Cranelift compiles raw WASM into Pulley bytecode;
the runner accepts only raw WASM from packages and never executes native JIT
code. Bazel precompiles Brush and the build-time composed default SysUI, UserUI,
and Dioxus demo components with the same engine configuration, then embeds their
compressed trusted artifacts and input digests in the runtime executable. The
decoder enforces an exact output length bounded to 32 MiB; native runner images
omit their static symbol table to reduce loading work. Only an exact SHA-256
match of the admitted raw WASM selects an artifact; updated and third-party
components use the normal compiler. Serialized package or checkpoint data is
never deserialized as code. Transplant preparation uses the same path and retains raw WASM as
the migration input. The BexOS adapter supplies owned VMAR-backed linear memories, execution
stacks, and per-thread engine TLS. It does not use libc memory or signal stubs
for guest memory or traps. Linear memories reserve their configured address
capacity but add adjacent lazy anonymous VMOs in 2 MiB chunks only as their
length grows, avoiding kernel page bookkeeping for unused capacity. A failed
growth preserves the previous logical length and all existing data. VMAR separation organizes mappings; WASM
validation, bounds checks, and checked host interfaces enforce the guest boundary.
The trusted runner and engine share the process address space.

Native service dispatches receive a fresh, bounded manifest fuel allowance on
each call; commands and child admission keep their existing finite budgets.
Migration snapshots are prepared for pending migration requests at safe points,
with exact record-change tracking. Ordinary idle dispatches do not encode the
component and application checkpoint, and a busy dispatch cannot expose a stale
checkpoint for cutover. Saved snapshots are released outside active migration.

The native runner is an optimized platform executable even in fastbuild images.
Appd embeds its SHA-256 digest and resolves the executable independently of the
consumer payload. Consumers cannot name an alternative native executable or
load manifest-declared native libraries into the runner. The launched process
retains the consumer identity, resource group, namespace, and service grants.
WASM drivers are rejected.

A guest replacement archive may carry a separately platform-signed runner archive
at `.bexos/wasm_runner.bex`. Appd verifies its signature, fixed
`bexos.platform.wasm_runner` role, native ELF path, and absence of process/driver
or native-library declarations. The guest cannot substitute an unsigned native
executable. This nested archive remains in the accepted package for subsequent
disk launches. Omitting it selects the platform default for both migration and
later launches.

## Configuration

Authored manifests remain prototxt:

```prototxt
package_name: "com.example.command"
name: "Example WASI command"
processes {
 name: "main"
 runner: "wasm"
 lifecycle { update_strategy: RESTART }
 runner_options {
  [type.googleapis.com/bexos.app.WasmRunnerOptions] {
   path: "/pkg/bin/main.wasm"
   arguments: "example"
   environment { name: "MODE" value: "batch" }
   limits {
    max_memory_pages: 256
    max_handles: 64
    max_children: 4
    fuel: 10000000
    fuel_slice: 10000
   }
  }
 }
}
```

Paths must be normalized under `/pkg`; traversal, empty segments, and NULs are
rejected. Arguments and environment entries are bounded and environment names
must be unique. Omitted execution limits use finite defaults:

| Limit | Default |
| --- | ---: |
| Input module/component | 16 MiB |
| Aggregate linear memory | 1,024 WASM pages (64 MiB) |
| Aggregate table elements | 65,536 |
| Guest stack | 512 KiB |
| Aggregate handles | 256 |
| Children | 8 |
| Execution fuel | 100,000,000 |
| Fuel per scheduling yield | 10,000 |
| Component argument lifting per host call | 128 MiB |

Compilation is serialized per runner process. Wasmtime also bounds cumulative
component argument lifting before host implementations receive nested lists or
strings. Host messages, stream buffers, and random requests have separate 32 KiB
caps; a child input may use its configured module-size limit. Shared memory,
guest threads, and 64-bit guest memories are disabled. Resource IDs are instance-local and never
expose a native handle or pointer. Core IDs are not reused after closure.

## Interfaces and child policy

The WASI bindings come from the pinned upstream WASI 0.2 WIT package. The command
world supports arguments/environment, stdio, streams/polling, preopened
filesystem directories, clocks, and socket adapters. Filesystem access resolves
through granted directories; network access requires a granted
`bexos.net.Netstack` endpoint. Unsupported filesystem/socket operations return
WASI errors. Randomness uses the kernel’s boot-seeded ChaCha20 stream and reports
unavailability if trusted boot entropy was not supplied. The stream position is
preserved in kernel checkpoints. QEMU supplies fresh host-OS entropy in a private
handoff file at launch; no random seed is stored in a reproducible build artifact.

Output streams retain bounded pending bytes and track write credits, partial
writes, flushes, and readiness. Their checkpoint state shares identity with output
pollables, so restored writes continue after the already-written prefix. TCP
listeners and UDP streams poll actual netstack readiness. Socket stream allocation
reserves all required resource slots before consuming an accepted connection.

Current WASI limitations return explicit errors: filesystem links, symlinks,
rename, timestamp mutation, metadata hashes, and exclusive creation are unsupported;
TCP/UDP tuning options and binding port zero are unsupported. IPv6 flow-info and
scope IDs must be zero. Stdin is closed when no input facility is available;
terminal handles are unavailable. The runtime provides UTC timezone information.
WASI 0.1 preview1 imports are not implemented.

`lib/wasm_runtime/wit/bexos.wit` defines `bexos:wasm@1.0.0` kernel, sandbox, and
lifecycle interfaces. Core modules use versioned imports under
`bexos:kernel/ipc@1.0.0`, `bexos:kernel/time@1.0.0`, and
`bexos:wasm/sandbox@1.0.0`. Service components import the same checked kernel
and sandbox facilities through generated component bindings. The same WIT package
now defines `bexos:wasm/ui@1.0.0`, `bexos:wasm/dioxus@1.0.0`, a `dioxus-library`
world for the shared UI component, and a `dioxus-app` world for applications that
import that shared component after appd and the runner compose the graph.

Child stores share the engine and aggregate budgets, with separate memories and
resource tables. Signed children may receive explicitly delegated subsets of
parent resources. Signature verification covers the immutable payload; an
invalid signature rejects spawning. The envelope is the bounded
`bexos.app.WasmChildSignature` protobuf: package ID, certificate chain, signature,
and algorithm (Ed25519 or ECDSA P-256/SHA-256). The runtime hashes the exact copied
payload and asks the existing AppTrustManager to validate it. Parent manifests
must request that service to admit signed children. Guest origin labels have no
authority. Unsigned children receive private
computation resources and bounded, handle-free parent messages. They cannot
receive filesystem, network, hardware, or arbitrary channel handles, including
later transfers. Unsigned children cannot receive filesystem, network, hardware, arbitrary channel,
or graphics/display grants. The Dioxus interface includes direct scene submission for renderer tests and `submit-document` for normal apps; the shared component resolves the supported document/CSS/layout/text subset before using the host UI scene interface. Dioxus UI apps receive graphics access only through
authenticated parent process grants checked by the native runner. JS packaging
remains future work.

Pausing retains the live instance. Resuming permits later invocation;
termination closes its resources. Status remains `terminated` for a previously
issued ID without retaining the child store. Invoking a paused or terminated
child reports an error.
Signed command-component children may use WASI only through their delegated
resource table. Child stdout/stderr use explicitly delegated writable file or
socket resources named `wasi:stdout` and `wasi:stderr`; otherwise they are closed
streams. Embedded stores do not inherit ambient WASI clocks or randomness; the
versioned BexOS monotonic-time hostcall remains available. Unsigned components are
rejected, while unsigned core modules retain bounded private computation and
parent messaging. Typed child delegation preserves authenticated service metadata
and method restrictions. Raw channel transfer rejects resources whose companion
handles or grant metadata cannot be represented by that wire format.

## Lifecycle and migration

Services initialize fresh resources in activation before their first checkpoint.
Restoration must adopt existing resources without opening files, sending messages,
or replaying external operations; activation runs only after successful cutover.
Core services must declare `HEART_TRANSPLANT` and export the version-1 service
hooks: `bexos-service-version`, `bexos-service-dispatch`, `bexos-checkpoint`,
`bexos-restore-allocate`, `bexos-restore`, `bexos-activate`, and `bexos-abort`.
Checkpoints describe application state, not an engine stack snapshot.

The transport adapter encodes checkpoints, resource identities, queued parent
messages, child trees, and remaining fuel. Compilation and store instantiation
for the replacement and live children happen during bulk preparation, while the
source is live. The final checkpoint is installed into those prepared stores
at quiescence. A changed child set or payload after preparation rejects migration;
cutover does not compile or silently discard a child. The candidate restores
before kernel activation; guest channel I/O is blocked during restoration.
Native endpoints are preserved by resource adoption. Missing hooks, invalid
state, exhausted budgets, or unsupported live resources reject migration.
A live command child rejects a service transplant. Dioxus UI migration additionally
requires the runner to drain outstanding GPU work and compositor release fences
before cutover, while the application checkpoints only serializable model state.
WASI descriptors, streams,
pollables, and socket state have checkpoint codecs. Component guests explicitly
identify typed resources at checkpoint time and adopt those tokens during
restore through `checkpoint-resources`; every live resource must be accounted
for. Host tests cover typed, single-use adoption and descriptor/stream metadata
round trips. QEMU verifies a component service that reads a byte sequence through
an open input stream before and after replacement, preserving its descriptor,
stream offset, pollable, and client connection. Component services use the versioned
`bexos:wasm/lifecycle@1.0.0` exports. Ordinary commands use restart
lifecycle. QEMU verifies core-service cutover, rollback on guest restore
rejection, live paused-child state, component descriptor/stream state, and
persistent client connectivity.

## Verification

The following host suite passed after Rust formatting:

```sh
bazel test -c opt //kernel/core:core_tests //lib/wasm_abi:tests //lib/wasm_runtime:wasm_runtime_tests //lib/userspace:userspace_tests //services/wasm_runner:wasm_runner_tests //services/appd:appd_tests //services/debugd:debugd_tests //services/netstack:netstackd_tests //services/trustd:trustd_tests //tools/qemu:qemu_tests //:heart_transplant_coverage_test
bazel run @rules_rust//:rustfmt
```

The runner, replacement archive, and product images also build explicitly:

```sh
bazel build -c opt //services/wasm_runner:wasm_runner_elf //services/wasm_runner:replacement_archive //device/virtual/qemu/nongui:virtual_aarch64_product_assembly //device/virtual/qemu/nongui:bootfs_image //device/virtual/qemu/nongui:qemu_nvme_disk_image
```

The QEMU suite builds the runner, guest fixtures, BootFS, and disk product images:

```sh
bazel test -c opt --test_tag_filters=requires-qemu //testing/e2e/qemu/wasm:wasm_e2e_test
```

The full suite passed, including command/child/trap execution, memory growth and
exhaustion, denied/granted TCP/UDP/DNS operations, rollback, live-child migration,
runner replacement, and component descriptor/stream migration. Test harnesses own
and stop their QEMU and RPMB processes.
The heart-transplant coverage target checks registration and replacement-archive
coverage; behavioral migration checks run in the runtime tests and QEMU suite.

Dioxus focused verification adds real component composition of the demo application
with the separately packaged `com.bexos.lib.dioxus` component, appd rejection of
native substitution for a component import, scene validation, Vello conversion,
CPU fallback replay, and a userspace AArch64 build of the Venus/Vello UI path.
The graphics suite includes `//testing/e2e/qemu/graphics:dioxus_smoke_aarch64`
and `//testing/e2e/qemu/graphics:dioxus_smoke_x86_64` to launch the packaged app
in the graphical workstation image and assert the native first-frame marker when
the image reaches scened, debugd, and the app-launch path.
QuickJS compilation/packaging and native JIT execution remain future designs.
