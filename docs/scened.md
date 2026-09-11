# Scened and shared Flatland libraries

The implementation includes CPU composition, an integrated Vello/Venus worker,
self-only scheduled CPU accounting, shared Rust/C allocator counters, and sustained
real-session workload fixtures. Repository validation results are recorded in
[scened repository validation](scened-validation.md), with the consolidated schema
3 report in `scened-completion-2026-09-08.json`. Functional/accounting validation
and complete sustained measurement windows pass; hardware 1080p/120 Hz acceptance
remains pending. The long-term designs remain in
[RFC 0047](rfcs/0047/README.md), [RFC 0042](rfcs/0042/README.md), [RFC 0045](rfcs/0045/README.md), and [RFC 0046](rfcs/0046/README.md).

## Accounting and sustained workload contract

`TaskControl.GetRuntimeStats` returns calling-thread and process CPU nanoseconds.
The scheduler charges scheduled execution, including the current running interval
and exited workers; blocked time is excluded. Snapshot readers accept the old
scheduler format and retain exited-thread totals in the new format. The call uses
fixed IPC buffers and grants no authority to inspect another process. See the
[ABI reference](idl-and-abi.md#self-runtime-accounting).

`bexos_libc::allocation_stats` counts successful logical allocation/reallocation
requests, requested bytes, frees, and failures at the boundary shared by Rust and
C. Aligned and direct-mapped allocations use the same counters. A moving realloc
counts once; its internal allocate/free is untracked. Temporary allocations made
by allocator IPC are real separate requests and count. Requested bytes measure
allocation traffic, not peak residency; VMOs created outside the allocator are
outside this heap metric. Counter snapshots and telemetry formatting allocate no
heap storage.

Schema 3 `scened-metrics` records identify diagnostic epochs and nonoverlapping
windows. Each epoch excludes 32 warmup completions, then reports every 256 measured
completions. Main-thread CPU spans consecutive completed-frame boundaries,
including IPC, preparation, input routing, rendering, completion handling and
wait setup/return. Process CPU includes the GPU worker. Main active-loop wall
time excludes the blocking wait and differs from both CPU and presentation
latency. `processing_p99_us` measures frame dispatch through driver completion;
it is not client-request-to-photon latency. Worker CPU uses consecutive completion
boundaries, including receive polling and completion notification; worker wall
time measures the individual job. Submission preparation and optional GPU queue
intervals are separate measurements. Allocation deltas span the same completion
boundaries. Fixed-buffer report formatting is outside the workload
samples; concurrent worker execution remains accounted for.

Private ShellControl ordinal 5 begins an idle measurement epoch for a selected
fixture workload. Replacement restarts diagnostic warmup and counters while
preserving logical presentation state and the existing worker-drain requirement.
Missing CPU or GPU timestamp samples emit `null` with an explicit sample count.
The collector still reads older reports, deduplicates repeated log sections, and
rejects conflicting window identities. Old logs do not acquire zero-valued CPU or
allocation measurements.

F1–F5 in the persistent input fixture select fullscreen composition, eight animated
overlapping surfaces, a moving 64×64 partial region, backdrop blur, and alternating
leased fullscreen scanout. The recipe is Bazel-generated from
`testing/e2e/qemu/graphics/input_fixture/package/performance.prototxt`. Each runs
32 warmup and 256 measured completions through real sessions at 800×600. Native
fixtures request CPU composition on AArch64 and x86_64; nested Venus requires GPU
composition for workloads 1–4 and actual direct scanout for workload 5. A silent
CPU fallback fails the GPU fixture. Completion counts, representative pixels,
partial-copy bytes and release-fence retirement are checked before success.

Resolved paint-order damage tracking compares old/new items and repairs exposed
pixels. Unfenced presentations conservatively invalidate mutable content. GPU
partial updates reduce readback bytes; Vello still rasterizes its full target.
Under image-atlas allocation pressure, Vello reclaims unused previous-scene
residency before growing the atlas and preserves images resolved by the current
scene. The shared GPU aperture remains 256 MiB.
Each workload has its own 8,790-second harness bound, derived from
`(32 + 256 + 1) × 30 + 120`; the extra completion retires the final lease. Existing
service recovery, transplant and input deadlines are unchanged. All VM wrappers
terminate and reap their fixture processes.

## Historical integration evidence

The measurements in this section predate the accounting and sustained fixtures;
they retain their original scope. Fresh outcomes and commands are in the
[validation record](scened-validation.md).

The preceding focused host matrix passed **37 Bazel test targets**, covering shared
Flatland, CPU rendering, layout/style/text, input, GPU transport, compositor
protocols, resource ownership, and migration. This includes 129 kernel tests and
regressions for SIMD blending, bounded archive decompression, scanout leases,
PCI root-port windows, process termination signals, and worker-thread exits during
transplant. Some unchanged targets reused successful Bazel test results.

The complete AArch64 input integration run passed in **477.8 seconds**. In one
run it verified all five replacements (GPU, PCI, input, compositor, and persistent
client), source survival after both rejected candidates, downstream PCI keyboard
attachment/removal, held-key cancellation and appd retirement, fullscreen imported
scanout, return to composition, delayed buffer retirement, and client disconnect
with identity revocation and a responsive surviving client. It captured 450
reference frames. The harness stopped QEMU on completion.

The run used a separate 90-second driver/package attachment budget and 30-second
input/removal budgets; guest scheduling and migration deadlines are unchanged.
Observed attachment was 21.636 seconds, injected-key delivery 75.724 ms, and removal
through key cancellation/registry acknowledgement 184.559 ms. These are single
host observations under TCG, including package loading, host polling, and UART
logging where applicable; they do not establish input-latency percentiles or
hardware performance. The final driver-dependency error-path handle cleanup was
written while this run was executing and is included in the next guest build.

Nested Linux runs verified Mesa/Vello submission, full/partial Dual Kawase
rendering, the common CPU/Vulkan scene and backdrop ordering, and multilingual
glyphs through Venus/Lavapipe. Text comparison found 2,291 ink pixels, 17,014 total
absolute channel error (1.86/255 mean), and no unmatched ink pixels. Both fixture
and bootstrap discovery routes now grant methods 7–15. Offscreen verification
finishes before normal boot readiness so temporary and compositor Vello workspaces
do not exhaust the 256 MiB aperture.

Worker warmup, partial repair, rejected-job buffer return, teardown, restart, and
GPU timestamp readback passed. One software run observed 10.686/12.035-second
warmups, 3.194/3.621-second profiled frames, and 9.635/10.523-second teardown times.
These are functional observations under TCG/Lavapipe, not performance acceptance.
The teardown fixture allows 30 seconds; compositor recovery and migration deadlines
are unchanged.

A nested run passed GPU, PCI, and input-driver replacement but failed during
compositor replacement. The kernel aborted the transplant when the
source renderer worker exited normally, retiring the candidate despite the live
source main thread. The fix limits this automatic abort to the last live thread.
Kernel regressions pass for normal worker exit in both source and candidate,
endpoint preservation, subsequent commit, and existing final-thread rollback.
Subsequent nested runs confirmed compositor cutovers of 99 ms, 79 ms, and
108 ms, worker reconstruction, and rejection of incompatible candidates. The
latest run completed the full replacement verifier, including the persistent
client and both rejected candidates. Fullscreen scanout and restoration of
composition passed. Software Vulkan exceeded the original two-second recovery
bound: scened retained display output and resumed CPU presentation, so the final
assertion requiring a GPU-composed frame correctly failed.

The desktop prototxt now explicitly configures `software_vulkan_timeout_ms: 30000`.
This applies only when Vulkan reports a CPU adapter, and is bounded to 2–30
seconds. Omission or zero retains 2 seconds; hardware GPU recovery always remains
2 seconds. The worker's completion wait and compositor recovery use the same
selected bound. This is a software functional-validation policy, not a relaxed
120 Hz acceptance target. Frame scheduling, input, and migration deadlines are
unchanged. All eight affected renderer/compositor host targets pass, including
configuration bounds, backward compatibility, and fixed hardware deadlines.
The complete nested fixture now passes with this policy in **858.8 seconds**.
It verifies actual compositor GPU-frame submission, restoration of composition,
retirement of the imported scanout lease, client disconnect and identity revocation,
and a responsive surviving client. GPU/PCI/input/compositor/client cutovers were
43/61/37/82/72 ms. The VM powered down and all helpers exited.

The nested verifier streams its progress and retains it on failure. It allows
480 seconds for five signed package preparations and two rejected candidates,
with a 1,140-second bound for the entire disposable VM. The guest Bazel target
uses the larger timeout class so the VM wrapper can perform cleanup itself.
The render-server diagnostic monitor tolerates tracees exiting between waitpid
and ptrace CONT, so an ordinary exit race cannot kill remaining render contexts.

The complete x86_64 input/transplant suite passes in 495.3 seconds, including all
five replacements, both rejected candidates, hotplug/removal, scanout restoration,
and client cleanup. All five graphical boot targets passed: standard AArch64
and x86_64, secure AArch64, and 1920×1080 on both architectures. These guest passes
precede the software-adapter policy change. Nongraphical boot also passes with the policy change: AArch64 in 54.7 seconds
and x86_64 in 59.2 seconds. Earlier passing results below apply to their
recorded revisions.

- `DisplayCoordinator` preserves ordinals 1–15 and adds scanout capabilities (16),
  VMO import (17), imported presentation (18), and release (19). Imports use the
  granted endpoint, ownership generation, read-only VMO capabilities, and retained
  DMA pins. Buffers must be tightly packed BGRX8/RGBX8 at the active display size.
  One scanout is used; additional visible surfaces are composited.
- Pixel formats 1/2 retain BGRA8/RGBA8 premultiplied semantics. Formats 3/4 are
  opaque BGRX8/RGBX8; their fourth byte is ignored. Direct scanout requires an
  opaque, unclipped, unscaled fullscreen top item without compositor overlays.
  It also requires a committed acquire/release lease. Legacy presentations can
  modify pixels in the same mapping and therefore compose/upload again instead
  of retaining an earlier direct-scanout upload. The host regression for this
  lifetime guard passes.
- Imported presentation avoids compositor pixel work and the driver's staging
  copy. VirtIO `TRANSFER_TO_HOST_2D` still uploads from guest backing to the host;
  this is not a claim of zero-copy host presentation. A command fence remains
  distinct from VSYNC and buffer retirement. The driver signals retirement only
  after a replacement `SET_SCANOUT` is acknowledged. Ambiguous device failure
  retains both possible leases and pins.
- GPU migration version 7 includes the selected display extent, imported resources,
  current scanout, retirement channels, and uncertain leases. It imports versions
  2–6 and the original format. Scened records version 10 adds CSS inputs to scanout,
  content effects/layout, and GPU endpoint
  state while preserving older record encodings until activation. Process-local
  layout, image, shader, and renderer caches are rebuilt.
- Flatland ordinals 21/22 stage content effects and layout. Content effects provide
  rounded bounds and a bounded backdrop radius; they do not replace ancestor clip
  semantics. Layout provides manual positioning, flex row/column, and equal-column
  grid with logical dimensions, growth, spacing, and padding. Taffy preparation
  runs before presentation queue acceptance. Resolved offsets/sizes travel with
  the atomic queued graph.
- CPU backdrop composition uses reusable scratch plus a sampling halo and copies
  only final damage to the output. Shared rounded geometry also gates input hits.
  The GPU worker now integrates Vello rounded clipping and ordered Dual Kawase
  backdrop passes. Scratch targets are retained and filtering includes the sampling
  dependency halo; up to 32 backdrops use this path before CPU fallback.
- The new optional Vello/Venus worker owns bounded GPU work and drains before
  migration. Shader setup and Vulkan completion waits stay off the input thread.
  Its current presentation bridge uses retained readback storage and alternate
  output mappings. It records copied bytes and worker duration; this is not a
  zero-copy GPU composition path or a measured 120 Hz result. Direct scanout is
  selected independently. CPU composition continues during GPU initialization
  and handles unsupported effects and renderer failures.
- GPU API and device-loss callbacks retain the first error for each device epoch.
  Workers check it before publishing output and return failed-job buffers instead
  of invoking wgpu's default abort handler. Host regressions cover error cascades
  and device-loss epochs. The compositor also stops waiting on an overdue GPU
  frame at its selected recovery deadline, resumes CPU frame processing, and
  discards late results. The worker retains its mappings and resources until it
  drains; transplant quiescence still requires worker retirement. Expired jobs
  carry no presentation sequences, allowing CPU feedback to advance safely.
  The deadline/migration host regression passes. The nested fixture verified
  deadline-triggered CPU fallback and continued scanout/composition. Hardware
  recovery stays at two seconds; CPU Vulkan adapters use the bounded prototxt
  policy described above. These bounds are separate from the 7.5 ms frame target.
- Shared `graphics_runtime::flatland::Session` and `graphics_runtime::scanout::Client`
  clients centralize protocol operations and retirement handling. Mesa notices
  are generated from pinned source/license inputs by Bazel and packaged with the
  compositor and fixture replacement archives, alongside nanoprintf's license.

Dioxus WASM UI clients use the shared Flatland content path through the native
WASM runner. The runner submits private BGRA VMOs with acquire/release fences,
tracks at most two outstanding releases per view, polls input from the same
session, and falls back from Vello/Venus GPU rendering to CPU scene replay without
changing the compositor protocol.

- Flatland ordinal 23 adds bounded selector IDs, classes, and inline declarations.
- Flatland ordinal 24 returns a session-local viewport for Dioxus and other
  client runtimes that need resize/scale information without receiving shell
  authority.
  A real Stylo tree adapter resolves selectors, inheritance, specificity, and the
  cascade only when logical style inputs change. The desktop prototxt contains the
  theme; text consumes computed size/color with metrics from pinned font tables.
  The initial scene projection supports pixel dimensions, flex direction, growth,
  uniform gaps/padding, opacity, uniform rounded corners, and typed equal-column
  grid. Unsupported dimension/spacing projections reject Present; full browser
  layout semantics are not provided.
- The GPU driver now selects the enabled primary display extent up to 4096×4096,
  including 1920×1080 when configured by QEMU. Allocation failure retains the
  800×600 boot fallback. Display dimensions survive replacement. The existing
  reference fixtures now explicitly request 800×600. Refresh timing still uses
  the identified timer fallback.

- Unsupported libc protection, signal-stack, CPU-topology/affinity queries, and
  thread naming now return explicit errors rather than false success or an
  invented two-CPU topology. The GPU worker uses the implemented native pthread
  path with an explicit stack and bounded work; no guard-page support is claimed.
  Native guest pthread/TLS/condition probes and the nested Vulkan worker
  lifecycle exercise this implementation; current suite status is above.
- Desktop prototxt `refresh_hz` selects 60 or 120 Hz (default 120). The
  scheduling profile, frame clock, and replacement use the selected period.
  Presentation record version 3 retains this period; in-flight frame version 3
  also retains the timer deadline. Earlier encodings remain byte-identical until
  activation, including the GPU worker's pending-frame record.
- Fixed-size diagnostic windows track main-thread wall time, frame completion
  latency, dispatch lateness, timer misses, and output/copy counters. Each process
  epoch excludes 32 warmup completions and emits `scened-metrics` JSON every 256
  measured completions. Worker duration includes Vulkan waits. Optional
  timestamp instrumentation has produced a valid counter readback in the nested
  software-Vulkan worker fixture: `profile_gpu: true` in the
  desktop prototxt requests supported wgpu encoder timestamps, retaining the query
  set and resolve storage and reading them with the existing pixel readback.
  `gpu_submission_p99_us` measures worker command preparation through submission;
  `gpu_queue_interval_p99_ns` is null when unavailable or profiling is disabled.
  Queue intervals include submission gaps and backend scheduling; they are neither
  shader busy time nor hardware VSYNC evidence. Profiling defaults off because it
  adds a timestamp submission. These process-local windows restart on replacement.
- New validation code compares shared CPU/Vulkan reference scenes within 3/255
  per channel away from a one-pixel edge neighborhood, requiring at least 85%
  full-frame comparison coverage. Backdrop ordering has a uniform-color reference
  with 2/255 tolerance. The existing mathematical Kawase test retains 3/255
  tolerance. These comparisons pass in the latest nested Linux/Venus run.
- Persistent-client fixture F10 transitions from styled composition to an opaque
  BGRX fullscreen buffer and back. Native and nested QEMU fixture code checks
  visible colors and delayed scanout lease retirement. These checks pass in
  the complete AArch64 input integration run.
- Host benchmark source now animates overlapping geometry and emits JSON with
  timings, allocations, affected destination bytes, and budget exceedances.
  `bazel run -c opt //testing/performance/scened:benchmark -- --output /tmp/scened-benchmark.json`
  captures machine details and raw workload results. Scanout eligibility in this
  host benchmark is a decision-cost measurement; guest fixtures exercise actual
  imported presentation. Measured results and their limits are recorded below.
- GPU output repair now tracks the stale region of the alternate output mapping
  independently from current display damage. Readback copies their union, or the
  full output after initialization, CPU fallback, direct scanout, or recovery.
  Backdrop scenes conservatively use full output damage; Vello still rasterizes
  its full target. This reduces readback work for ordinary partial updates without
  claiming incremental GPU rasterization.
- The private PCI registrar endpoint and MMIO allocation cursor survive live
  replacement. A read-only rescan of bus 0 and configured downstream buses runs every 250 ms and registers newly
  discovered VirtIO input functions through appd's normal installed-driver policy.
  Physical removal terminates the input process before releasing retained grants;
  the report endpoint closing cancels held input in scened. Failed removal retains
  the registry entry for retry. Direct PCIe root ports now support up to 15
  downstream buses with isolated 16 MiB windows; nested bridges/switches and guest
  ACPI eject remain unsupported. Root-port configuration happens at cold boot;
  runtime discovery and replacement preserve live BARs and bridge windows.
  Cold initialization also enables PCIe slot power: QEMU resets empty slots
  powered off, so bus numbering alone does not expose a subsequently added
  device. The slot-power regression and guest discovery/key delivery pass. The
  guest also observed key cancellation on removal, but appd's retirement wait
  exposed a kernel signal bug: process objects never reported `TERMINATED`.
  Process signals and final-exit wakeups are corrected, with passing forced/
  natural-exit and snapshot regressions. Guest cancellation and appd retirement
  now pass together.
  Hotplug now runs before replacements so device-discovery failures surface earlier.
  PCI runtime record version 2 retains root-port windows/cursors and reads version
  1. Existing single-bus ECAM mappings remain valid for version 1; downstream
  records require the 16 MiB ECAM mapping. Admission checks bus/device/function
  identity and rejects overlapping or out-of-range windows.
  The QMP fixture exercises keyboard attachment and surprise removal, with a
  held key and endpoint-specific cancellation checks. Complete guest removal
  and registry retirement pass on AArch64 and x86_64. The nested run also passes
  these checks and the complete integration fixture.
- Input test images include the normal installed input archive, PCI replacement,
  and deliberately rejected GPU/scened candidates. The rejection probe checks
  that package admission succeeds, receiver rejection retains the committed
  generation and source PID, and no candidate remains running. These checks run
  while the existing fixture holds keys, touches, and an unsignaled presentation.
- CSS equal fractional grid tracks and bounded `repeat()` project to typed grid
  column counts. Unequal, fixed-length, automatic-repeat, and more than 64 tracks
  reject the transaction instead of silently changing the requested layout.
- Fixture format 10 retains the deliberate client-disconnect phase. Closing an
  embedded client with a held key checks lease retirement, identity revocation,
  the surviving client's IPC, and removal of the old pixels. The shared Vulkan
  probe now compares Latin, Arabic, and Devanagari glyphs from one Parley layout:
  mean absolute channel error over the union of ink is at most 24/255, and at most
  2% of ink pixels may lack matching ink within one pixel. Blank output cannot
  pass via background coverage. The disconnect checks pass in AArch64 QEMU; the multilingual Vulkan comparison
  passes in the nested Linux fixture.

Local integration now passes. Full compositor CPU/allocation profiling and
sustained GPU workload performance acceptance remain outside the measured evidence;
hardware 120 Hz performance is unverified.

## Consolidated measurement report, 2026-09-08

[The combined report](scened-benchmark-2026-09-08.json) includes optimized host
workloads and the successful nested guest log. The collector preserves units,
removes duplicate printed observations, rejects malformed/nonfinite values,
and requires explicit success markers plus a successful VM exit. Its four host
checks pass. It does not infer hardware acceptance from functional completion.

With QEMU and helpers stopped, Apple M2 / 8 GiB / macOS 26.5.2 host measurements
(10 warmups, 200 samples) gave these operation-wall-time p99 values:

| Workload | p99 | Allocations over measured samples |
| --- | ---: | ---: |
| Fullscreen CPU composition, 1920×1080 | 0.891 ms | 0 |
| Eight animated translucent 640×480 surfaces | 1.682 ms | 0 |
| Partial 64×64 damage | 0.012 ms | 0 |
| Box backdrop blur, 640×480 | 0.878 ms | 0 |
| Fullscreen box backdrop blur | 6.004 ms | 0 |
| Cached glyph rasterization, 480×120 | 0.047 ms | 1,629 |
| Changing multilingual text and rasterization | 0.110 ms | 5,925 |

The report also contains two unique software GPU worker timing observations,
one input hotplug/latency observation, and five cutover durations. The functional
run did not produce a complete 256-frame compositor metrics window after warmup;
its report leaves that collection empty. Guest allocation counts and complete
compositor CPU accounting are explicitly unavailable. CPU output extents are not
aggregate memory traffic; the scanout microbenchmark measures eligibility only.
Fullscreen CPU blur still exceeds the 3 ms target in this host run. These limits
prevent treating this evidence as full compositor or hardware 120 Hz acceptance.

```
bazel test -c opt //testing/performance/scened:telemetry_tests
bazel run -c opt //testing/performance/scened:benchmark -- --output /tmp/scened-report.json --guest-log bazel-testlogs/testing/e2e/qemu/venus_linux/guest_aarch64/test.log
```

The guest log must come from the preceding Venus fixture run. Host benchmarks
must run after its VM/helpers exit. The earlier results below remain as a baseline.

## Optimized host measurements, 2026-09-07

Matching premultiplied surface rows now use four-pixel NEON (AArch64) or SSE2
(x86_64) source-over, with scalar tails and the existing transformed/converted
pixel path. AArch64 host tests compare every alpha/opacity pair, saturation,
unaligned rows, and tails against the integer reference. The complete AArch64
input integration also exercises this path; the latest x86_64 input suite passes.

The follow-up [SIMD measurement](scened-benchmark-simd-2026-09-07.json), on the
same machine with QEMU stopped, reduced the eight-surface workload from
5.876/7.000 ms p50/p99 to **1.477/1.583 ms**, with no measured iteration over
3 ms and no allocations. Partial 64×64 damage fell from 0.049/0.064 ms to
0.012/0.012 ms. Fullscreen CPU blur remains above 3 ms (5.717 ms p99 in that
run), and glyph rasterization still allocates. These are host operation timings,
not compositor-wide or hardware display acceptance measurements.

The table below retains the pre-SIMD baseline for comparison.

Measured with `bazel run -c opt //testing/performance/scened:benchmark` on Apple M2,
8 logical CPUs, 8 GiB RAM, macOS 26.5.2, with QEMU stopped. Each workload has
10 warmup iterations and 200 measured iterations. Values are operation wall time,
including host descheduling, excluding service IPC and physical display timing.
[Raw machine details and results](scened-benchmark-2026-09-07.json) are retained.

| Workload | p50 | p99 | Allocations over 200 iterations |
| --- | ---: | ---: | ---: |
| Fullscreen CPU composition, 1920×1080 | 0.763 ms | 1.000 ms | 0 |
| Scanout eligibility decision only | below timer resolution | 42 ns | 0 |
| Eight animated translucent 640×480 surfaces | 5.876 ms | 7.000 ms | 0 |
| Eight surfaces, 64×64 output damage | 0.049 ms | 0.064 ms | 0 |
| Box backdrop blur, 640×480, radius 16 | 0.908 ms | 1.265 ms | 0 |
| Box backdrop blur, 64×64 damage, radius 16 | 0.016 ms | 0.016 ms | 0 |
| Box backdrop blur, 1920×1080, radius 32 | 5.530 ms | 6.161 ms | 0 |
| Cached multilingual shaping | 42 ns | 84 ns | 0 |
| Cached glyph rasterization, 480×120 | 0.050 ms | 0.055 ms | 1,629 |
| Changing multilingual shaping and rasterization | 0.085 ms | 0.146 ms | 5,926 |

Overlap uses premultiplied BGRA pixels `[64,96,128,192]`, node opacity 0.8,
100×50-pixel offsets between surfaces, and an animated first surface. Partial
damage is at (400,300). The text contains Latin, Arabic, and Devanagari with the
pinned Noto fonts at 24 pixels. Text rasterization retains its glyph atlas but
still allocates; unchanged compositor text reuses the rendered pixel cache.

In the pre-SIMD baseline, overlap and fullscreen CPU blur exceeded the 3 ms CPU
target in all 200 samples. The SIMD results above replace the overlap baseline;
fullscreen CPU blur still exceeds that target. These numbers are not compositor-wide
CPU accounting or hardware 120 Hz acceptance. The fast scanout decision excludes
the actual guest import, submission, and retirement path. Reported output bytes
describe the destination damage extent, not aggregate memory traffic or overdraw.
These CPU benchmarks exclude GPU timestamps and physical display/input timing.
Software Vulkan queue intervals are recorded separately in the integration status.

## Implemented service behavior

Flatland method ordinals 1–9 retain their existing wire format. Mutations edit a
session's pending graph. Present validates and snapshots that graph into a queue
of at most three transactions. Later mutations cannot change a queued frame.
Presentation times use kernel monotonic ticks and must not decrease. At a due
frame boundary, scened latches at most one eligible transaction per session.
Acceptance is not a display-completion fence. `PresentWithFences` adds explicit
acquire/release ownership. The acquire channel receives exactly one empty,
handle-free message before latching. Each referencing presentation holds a
buffer lease until its committed graph is superseded or removed. Clients must
wait for all referencing presentations to release before changing their pixels.
Legacy `Present` remains available but provides no explicit reuse fence.

| Ordinal | Added method | Behavior |
| --- | --- | --- |
| 10 | ReleaseTransform | Removes a pending node and parent links; children remain detached |
| 11 | RemoveChild | Removes a pending parent/child link |
| 12 | ClearContent | Removes pending content; committed/queued references retain the mapping |
| 13 | SetChildOrder | Reorders a parent's children; later children paint above earlier children |
| 14 | GetPresentationInfo | Reports accepted sequence, queued count, and last latched time |
| 15 | ReadInput | Reads at most eight normalized events for the calling session |
| 16 | PresentWithFences | Accepts acquire/release channels and returns an accepted sequence |
| 17 | GetViewReference | Returns an opaque channel capability and its stable view identity |
| 18 | SetViewContent | Stages capability-authorized child view embedding on a node |
| 19 | ClearViewContent | Detaches pending embedded content |
| 20 | GetPresentationFeedback | Reports latched, completed and rejected sequences, completion ticks and scene generation |

Acquire readiness is polled without waiting; an unsignaled frame cannot block
other sessions. Malformed or closed acquires cancel their queued frame and
release its lease with an error. Release messages carry sequence and status;
a successful release means retirement from the current CPU composition path,
whose display driver keeps a separate copy. This is neither hardware VSYNC nor
GPU submission completion. Disconnect retires leases after dropping scene and
queue references. There are at most 64 live fenced presentations across all
sessions (three queued and one committed per session).

Each granted endpoint owns a session. Disconnect removes its pending/committed
scene, queued transactions, and geometry cache, then releases unreferenced
mappings. SetContent duplicates and maps the transferred VMO once; rendering
borrows that mapping. Graphs cannot refer to other sessions' transform nodes.
Embedded views inherit their parent node’s transform, clip and opacity. Paint
order is parent content, embedded view, then the parent node’s children. A view
has one embedding parent; cross-view cycles and depths over 256 are rejected
before cache mutation. Conflicting queued presentations are resolved at the
frame boundary against the latest committed topology, with explicit rejection
feedback. A previously embedded view stays hidden after detach or parent loss
until it is attached again. Rendering, input and accessibility share this world
geometry and paint order.

The compositor admits 16 sessions, 256 nodes per session, 512 total retained
content mappings, and 256 MiB of mapped content. Migration stream identities
also require handles in `1..=u32::MAX/2`.

A resolved snapshot contains flattened stacking, accumulated translation and
positive scale, rectangular clipping, opacity, visibility, and surface bounds.
Rendering and the shared hit-testing library use the same representation.
Scened traverses only sessions that latch transactions and caches each node’s
local and inherited geometry. Unchanged nodes reuse their resolved geometry and
inverse scales; ancestor changes invalidate their affected descendants. Paint
order updates independently of geometry. The retained traversal buffers and
bounded stack topology validation allocate nothing after warm-up for updates
to existing nodes. Scale/opacity/graph validation
rejects cycles, multiple parents, non-finite accumulated geometry, and invalid
surface layouts.

CPU composition performs no surface-sized clones. Pixel loops cover the
intersection of content visibility and damage; translated unit-scale surfaces
use contiguous spans. Damage covers old and new session bounds. The output
mapping is retained and unchanged compositions are skipped. Each of the two
VirtIO-GPU back buffers tracks its own stale region; recycled buffers repair
that region before applying new damage. The composition presentation path copies into driver DMA backing. The separate
imported-buffer path avoids compositor redraw and driver staging, subject to the
scanout eligibility and lease rules above.

The shared CPU box-blur fallback admits radii up to 32 pixels and widths up to
4096 pixels. It filters in place using a reusable row ring and channel sums;
scratch storage is at most 1,130,496 bytes. Only the requested output rectangle
and its sampling halo are visited. Tests compare replicated-edge filtering
against an exact square reference (at most one channel value difference),
verify unchanged output padding and surrounding pixels, and check zero frame
allocations. Scene effect properties and backdrop damage integration use retained
scratch and sampling halos; service damage remains conservative at view bounds.
`//lib/flatland:blur_benchmark` measures full, window-sized, and
64×64 partial regions on a 1920×1080 target; these are CPU microbenchmarks.

Scened requests 3 ms capacity, a 7.5 ms deadline, and an 8.333 ms period, without
exclusive CPU affinity. Its logged timing source is a 120 Hz **timer fallback**.
The shared clock/profile code retains 60 Hz support, used by splashd. Missed
frames advance to the next deadline without replaying intermediate deadlines.
IPC work is bounded by the admitted client count. Scened now submits a frame
without waiting for its display response, preserves the submitted per-view
sequences, and continues input and client IPC while the output is held. New
transactions latch after that response. The driver advances transfer, scanout
and flush through nonblocking completion polling; bootstrap and raw capset
queries still use synchronous commands. A GPU completion fence is not hardware VSYNC.
These asynchronous paths passed host validation and AArch64 (267.5 s) and
x86_64 (298.0 s) input/transplant runs. An earlier x86_64 takeover failed;
splash now reports busy while progress is below its handover threshold, and
scened retries for at most two seconds. This handles progress/readiness arriving
on independent channels; the failed run did not identify its precise rejection
branch. Handover deferrals now include that state in diagnostics.
The existing acknowledged splash takeover and retained-frame transplant remain.

VirtIO-GPU now reads the device feature/configuration registers, display info
and capset metadata. DisplayCoordinator ordinals 7–9 expose offered/negotiated
features, display rectangles and bounded raw capset reads. Counts, sizes and
coordinates are validated; offered Venus support does not claim an enabled
Vulkan backend. When VIRGL, BLOB and CONTEXT_INIT are all offered, the driver
negotiates those features. The private `GpuTransport` capability grants
DisplayCoordinator ordinals 10–14: context creation/destruction, bounded command
submission, shared-memory allocation and resource release. A granted endpoint
owns its context IDs and resources. Admission is limited to 16 contexts, 64
resources, 64 MiB per resource and 256 MiB total. Device commands complete
asynchronously; a well-formed device rejection does not invalidate the queue.
Context and host-visible memory operations, CPU timeline fences, busy-reference
rejection and release passed against QEMU 11.0.3/virglrenderer 1.3.0. A BexOS
fixture submits `vkEnumerateInstanceVersion` through its Venus context and
checks the host Vulkan reply in the mapped blob. This validates transport and
shared-memory coherence; it does not constitute a linked Mesa/wgpu compositor.

GPU record version 5 preserves context/resource identities, guest DMA pins,
host-visible aperture allocations, borrowed host VMOs, and unmapped device-only
blobs. Host tests reject fabricated mappings on device-only blobs. Version 4
imports its mapped resources as host-visible. Versions 2–4 and the
previous unversioned component payload retain their encoding until activation.
Driver quiescence drains both presentation and transport requests. Host-owned
blobs cannot be released while client VMO handles or mappings remain; the
kernel's GPU-only shared-device VMO interfaces retain normal cached attributes
without making host RAM reclaimable guest memory. Kernel snapshot version 18
retains this backing type and timed futex deadlines, and imports prior versions,
including versions 16–17. Version 19 additionally preserves fresh endpoint
identities while recycling closed channel, capability and VMO storage, and imports
version 18. Live endpoint identities remain unchanged across kernel replacement.
Host tests cover mapping attributes, last-reference checks, rejected executable
mappings, snapshot restoration and safe reclamation. Guest host-blob validation
passed before and after driver/compositor/client replacement.

Earlier discovery-only coverage: Discovery, C mutex contention and retained capabilities passed
AArch64 QEMU (309.3 s) and x86_64 (257.1 s). One earlier x86_64 run hit
the cutover deadline and rolled back to the running source. Fixed-size
migration replies now avoid a 65,500-byte allocation, and the fixture polls
status without repeatedly dumping process inventories during cutover; the
rerun retained the same deadline.

The C platform layer implements plain pthread mutexes, once, condition variables,
relative futex deadlines, and timed mutex/condition waits through shared,
allocation-free futex algorithms. Unsupported mutex attributes fail explicitly.
Condition waits reacquire their mutex even after timeout. Kernel deadline wakes
are prompts to recheck the predicate; libc checks the absolute deadline.
Futex wakeups remain process-scoped. Scheduler snapshots preserve sleeping futex
addresses and deadlines, including clearing runtime block state after expiry.
Host tests cover broadcast, a signal racing sleep, timeout reacquisition, malformed
attributes, and migration. Both QEMU guests reached the condition-variable
signal/broadcast/deadline assertions. The AArch64 suite passed in 342.4 s. The
x86_64 run failed its overall 420 s harness budget after completing all four
replacements, before final input validation. Wake reconciliation now traverses
tasks once instead of searching by ID for every blocked thread; the serial probe
also omits a duplicate post-replacement inventory. The expanded functional
harness has a 600 s overall bound, with unchanged guest cutover deadlines.
The updated x86_64 input/transplant test passed in 356.6 s after the C stack
and blocking-join changes; GPU/input/scened/client cutovers were 39/39/84/69 ms.

GNU pthread keys use the correct four-byte ABI, generation-tagged reuse, and up
to four destructor passes. Errno and key values are thread-local. These checks,
mutex contention, and the input/transplant suites passed in AArch64 QEMU
(311.4 s) and x86_64 QEMU (353.5 s) before the deadline extension.

Mesa 25.3.0, its generators, and GNU/UAPI compile-time headers are pinned. Bazel
has compiled Venus C objects, the Vulkan lite runtime, and the initial BexOS
adapter. The adapter, explicit ash/Vulkan entry linkage, reusable bounded RPC client,
and queue/fence/readback fixture pass linked guest Vulkan execution and teardown.
The integrated worker now uses this adapter for scened's Vello GPU composition;
the complete expanded nested fixture now passes, including compositor-owned GPU
presentation and post-transplant scanout/composition transitions.

`//lib/venus_wgpu` compiles for BexOS AArch64 and constructs wgpu through HAL
from that explicit Venus entry, using the actual supported instance extensions.
A pinned wgpu HAL patch permits offscreen devices without `VK_KHR_swapchain`
and reports no surface capabilities for those devices. Device-memory allocator
blocks stay within the driver's 64 MiB resource bound. The nested Linux fixture
now verifies device creation, Vello compute rendering, texture readback and
resource teardown through this bridge.


## Shared libraries and integration status

| Bazel package | Implemented scope | Integration limit |
| --- | --- | --- |
| `//lib/flatland` | Transactions, geometry, hit testing, SIMD CPU composition, presentation queues, bounded CPU box blur | Integrated through graphics compatibility exports; CPU blur uses bounded box filtering |
| `//lib/flatland_layout` | Retained Taffy flex/grid tree with validated topology | Desktop insets and client transaction layout; typed equal-column grid projection |
| `//lib/flatland_style` | Stylo tree matching, cascade, retained stylesheet revisions; imports disabled | Prototxt theme and client style inputs resolve outside the frame loop; supported property projection is bounded |
| `//lib/flatland_text` | Parley font registration, fallback, bidi, shaping, wrapping, bounded entry cache | Compositor text uses pinned Noto fonts; application widgets own their rendered content |
| `//lib/flatland_cpu` | Retained Vello CPU glyph atlas and rasterization, premultiplied format conversion | Cached compositor text; surface composition and CPU effects use shared Flatland routines |
| `//lib/flatland_render` | Vello/wgpu renderer, shared surface/text encoding, retained Dual Kawase textures and damage bounds | Integrated GPU worker awaits the expanded guest run; output uses retained readback |
| `//lib/flatland_input` | Bounded queues, keymaps/repeat/modifiers, pointer acceleration, tablet/touch, focus/capture/cancellation, migration | Integrated routing and reserved-edge takeover; downstream hotplug guest validation is running |

Native graphics dependencies use an isolated Bazel crate universe to keep their
`std` features out of the kernel's `no_std` dependencies. FIDL and dependency
build scripts, including Stylo properties and Vello shader generation, run
through Bazel. Generated source files are not checked in.

Noto Sans, Noto Sans Arabic, and Noto Sans Devanagari are pinned to google/fonts
commit `5e35378e6bda803962ee6fd257e444a7d459660d`, each with a checked SHA-256 and
its SIL Open Font License file. `//lib/flatland_text:fonts` exposes these inputs;
text tests register them explicitly. Host font discovery is disabled. Text
cache reuse avoids re-shaping identical text/style; registering a font clears
cached layouts. The compositor embeds the three pinned fonts, and both initial BootFS packaging
and replacement archives include their licenses. `services/scened/package/desktop.prototxt`
defines multilingual desktop text, size, color and insets. Bazel runs protoc to
compile this configuration. Taffy lays out the desktop text and Vello CPU
rasterizes Parley glyphs into a retained surface before normal presentation.
Glyph atlas pages are limited to 512×512 with at most eight pages. Font
shaping and rasterization stay outside the steady-state frame loop. A
replacement rebuilds this cache on its initial global bulk record and retains
it through final deltas. Host rasterization/configuration tests and graphical
guest tests on AArch64, secure AArch64 and x86_64 passed. A Bazel-applied
dependency patch exposes atlas dimensions because Vello CPU’s default
4096×4096 page exceeds the guest VMO allocation limit; the shared rasterizer
uses 512×512 pages with matching packing coordinates.

## Input service behavior

`bexos.driver.input.virtio` binds PCI VirtIO input devices. Each instance owns
64 preallocated event descriptors and a separate status queue. The driver
publishes a private `InputDevice.Subscribe` channel carrying bounded batches
of at most 64 Linux input events, a sequence number, and an explicit reset flag.
Startup discovery supplies up to 16 device grants to scened. Small channel
writes, wait requests and report reception use bounded stack buffers instead
of allocating 64 KiB RPC scratch buffers on every poll. Reports use a
1 ms timer polling fallback; hardware interrupt delivery is not implemented.

Scened normalizes reports and routes pointers using committed clipping,
stacking and inverse transforms. A down event captures its recipient and sets
keyboard focus. Changing focus releases held keys to the old view and suppresses
those held keys until physical release. Keyboard input goes only to the focused
view. Tablet coordinates use advertised ABS ranges; multitouch supports 32
slots. US translation is the default, including modifiers and a 500 ms initial
repeat delay followed by 33.333 ms repeats. Keymap tables and acceleration are
logical library state initialized from the Bazel-compiled input prototxt. The
configuration bounds repeat timings, acceleration, and keymap entries.

Each view has a 128-event queue. Consecutive pointer motions coalesce. Overflow,
malformed report recovery, stream discontinuity and device removal cancel or
reset affected input, preventing stuck captures and keys. Client reads never
wait for new input. Scene removal cancels captures without waiting for another
hardware event; failed reply delivery resets the affected client stream. The
driver resets the device with a bounded status check before acknowledging
lifecycle removal, then closes its report stream. Appd replacement and input
heart transplant retain live device queues. PCI discovery and appd attachment
wire newly plugged devices into the compositor; downstream root-port removal
and cancellation pass in AArch64 and nested Linux fixtures.

The dedicated `//device/virtual/qemu/input_test` product includes a
transplant-capable fixture with two persistent Flatland sessions. Its QEMU
tests inject real VirtIO keyboard, mouse, tablet and multitouch reports; guest
validation status is recorded below.

## Private compositor controls

`GetViewReference` returns a channel token with only TRANSFER and DUPLICATE
rights. Kernel `ObjectControl.GetChannelIdentity` (ordinal 11) identifies an
owned endpoint and its peer; identities survive duplication and kernel
snapshots. Numeric IDs grant no authority. Scened resolves every incoming view
token through this kernel operation and its live owner registry. Disconnect
invalidates the registry entry even if another process retains a token.

Appd privately grants `AccessibilityControl` and `ShellControl`. The former
queries committed view transforms, clipping, visibility, and view/scene
generations; controls a local-coordinate focus ring; and sets magnification
and color filters. A stale view generation cannot set a ring. The CPU path
supports grayscale, inversion, high contrast and bounded linear protanopia/
deuteranopia approximations. These approximations are not calibrated display
color management. Magnification uses a retained source mapping and inverse
coordinate mapping for input. The shell can focus and raise a token-identified
visible view. These interfaces contain no semantic tree data; `a11yd` remains
future work.

Control capabilities, issued tokens, focus-ring geometry, display settings,
stacking and generations are versioned logical migration state. Pixel scratch
is process local. AArch64 QEMU passed the expanded fixture in 239.7 s: it checked
unrelated-token denial and stale-generation rejection, observed the magnified
grayscale scene and focus-ring pixels, then verified the same view references
after scened replacement during held keyboard/touch input. The later combined
AArch64 and x86_64 runs also cover these checks and embedded views.

## Heart transplant compatibility

The graphics runtime supports component-defined record sets while preserving
the default one-record adapter used by splashd and VirtIO-GPU. Scened stores a
versioned global record, bounded fence and control records, plus streams keyed by session endpoint or mapping
handle. Each stream has length/checksum metadata and 16 KiB chunks. A session
stream includes pending and committed graphs, presentation times, queue
sequence, and every queued transaction. Buffer streams contain logical mapping
descriptors. Record data stays below the 32,704-byte transport limit.

Candidates validate complete streams after the final delta prefix and reject missing,
extra, or checksum-invalid chunks. Bulk copy may contain mixed revisions and is
not assembled prematurely. Tombstones remove session/resource streams.
Global record version 3 added fence record 2, containing waiting/ready/committed
leases and their channel capabilities. Version 4 added control record 3; version
5 adds embedded view links and presentation feedback to session streams.
Version 6 adds record 4 for an outstanding compositor display submission, its
deadline and original view sequences. Record 4 version 2 also preserves the
bounded splash readiness retry deadlines. The output mapping remains unchanged
until the retained display endpoint receives completion. Versions
2–5 and the old monolithic compositor record are accepted, retaining their old
key sets and graph encoding until activation. Control record version 3 preserves
pending frame-boundary display, focus-ring and stacking changes; earlier control
versions 1–2 remain readable. Owned mappings and granted client endpoints survive transfer.
Resolved geometry and driver damage caches rebuild after activation; no Rust,
Stylo, Parley, Mesa, or wgpu pointers are serialized. The source remains separate
from candidate decoding. The GPU driver stops accepting new work when migration requests quiescence
and polls the outstanding frame to completion before entering cutover. No
process-local command or renderer pointers are serialized. GPU record version 5
also preserves the logical Venus transport state described above. Mesa/wgpu
worker caches drain before source quiescence and rebuild in the activated target,
without serializing process-local pointers.

Input aggregation occupies reserved chunk stream 1. It preserves device/report
endpoints, report sequence, keymaps, held keys, repeat deadlines, pointer state,
32 touch slots per device, focused view, captures, blocked held keys after focus
changes, and queued client events. Input stream version 2 adds reserved-edge
recognizer progress, its owning shell endpoint and a bounded shell queue.
Claiming a stream queues cancellation for the original view before shell Down/Move;
only the shell receives the remaining stream. Overflow resets the shell stream
and suppresses orphaned releases. Private ShellControl ordinals 3 and 4 configure
edge bits/inset/threshold and read up to eight events. A disconnected shell
releases its reservation. These paths passed live replacement on AArch64
(258.3 s) and x86_64 (238.2 s), while holding a claimed gesture, another touch,
a key and an unsignaled acquire fence.

Input stream versions 3–4 additionally preserve configured acceleration, keymap
overrides, repeat timing and an in-progress nonblocking device subscription.
Device records version 3 adds repeat timing while accepting versions 1–2.
The source configuration is `services/scened/package/input.prototxt`, compiled
by Bazel/protoc. Its defaults are US layout, acceleration 0.25, a 500 ms repeat
delay and 33.333 ms repeat interval. These policy/subscription changes passed AArch64 (254.8 s) and x86_64
(255.3 s) QEMU runs with input and live replacement. The input driver serializes versioned queue
indices, DMA mappings/pins, PCI mappings and report-stream state. Replacement
adopts these resources without resetting the device or serializing queue pointers.

## Validation and measurements

Focused host suites cover transaction isolation/order/capacity, clipping and
transforms, borrowed composition, damage and back-buffer repair, layout
invalidation, multilingual shaping, stylesheet revisions, input queue overflow,
focus isolation, protocol encoding, and migration compatibility/corruption.

```
bazel test -c opt //lib/flatland:tests //lib/flatland_input:tests //lib/flatland_layout:tests //lib/flatland_style:tests //lib/flatland_text:tests //lib/graphics:tests //lib/graphics_runtime:tests //services/scened:tests //drivers/d1/display/virtio/gpu:tests
bazel test -c opt //lib/flatland:cache_allocations //lib/flatland_cpu:tests //lib/graphics_runtime:idle_allocations
bazel test -c opt //drivers/d1/input/virtio:tests //services/appd:appd_tests //kernel/core:core_tests
bazel test -c opt //lib/flatland_render/... //lib/venus_transport/... //lib/virtio_gpu_protocol/...
bazel test -c opt //drivers/d1/bus/generic/pci:pci_root_bus_tests //drivers/d1/bus/generic/pci:runtime_tests
bazel build -c opt //lib/flatland_render //services/scened:scened_elf
bazel test -c opt --config=e2e //testing/e2e/qemu/venus_linux:guest_aarch64
bazel test -c opt --config=e2e //testing/e2e/qemu/graphics:all_architectures
bazel test -c opt --config=e2e //testing/e2e/qemu/graphics:input_aarch64 //testing/e2e/qemu/graphics:input_x86_64
bazel test -c opt --config=e2e --test_env=BOOT_UI_NO_GPU=1 //testing/e2e/qemu/graphics:boot_ui_aarch64
bazel test -c opt --config=e2e //testing/e2e/qemu/kernel:kernel_development_smoke_test_x86_64 //testing/e2e/qemu/kernel:kernel_smoke_test_aarch64
bazel run -c opt //lib/flatland:benchmark
bazel run -c opt //lib/flatland:blur_benchmark
bazel test -c opt //lib/flatland:blur_tests
bazel run -c opt //lib/flatland_cpu:benchmark
bazel run @rules_rust//:rustfmt
```

The composition benchmark uses 1920×1080 output, ten warm-up frames and
200 samples per workload: one fullscreen surface, eight overlapping 640×480 surfaces, and a
64×64 damage rectangle over those surfaces. It reports nearest-rank host operation wall-time p50/p99 and
allocation counts around composition only. Scene creation, snapshot compilation,
background clear, IPC, driver copies, GPU time, input latency, and display
latency are excluded. It is not the hardware acceptance benchmark.

The separate CPU text benchmark uses a 480×120 text surface, ten warm-up
iterations, and 200 samples. It reports cache-hit shaping, glyph-atlas reuse,
and changing multilingual text with shaping plus rasterization. Font loading,
destination copies, IPC and display timing are excluded. This benchmark does
not establish a 1080p composition budget.

Measured on 2026-09-07 on Apple M2, 8 GiB RAM, macOS 26.5.2 (25F84), with Bazel
`-c opt` and no other Bazel build active during sampling:

| CPU-only workload | p50 | p99 | Allocations in 200 compositions |
| --- | ---: | ---: | ---: |
| Fullscreen opaque surface | 0.791 ms | 0.960 ms | 0 |
| Eight overlapping opaque surfaces | 1.005 ms | 1.032 ms | 0 |
| 64×64 partial damage | 0.0081 ms | 0.0090 ms | 0 |

| CPU text workload | p50 | p99 | Allocations in 200 iterations |
| --- | ---: | ---: | ---: |
| Cached multilingual shaping | 0.042 µs | 0.042 µs | 0 |
| Cached glyph rasterization, 480×120 | 36.500 µs | 42.250 µs | 1,629 |
| Changing multilingual shaping and rasterization | 68.334 µs | 125.375 µs | 5,927 |

The glyph atlas avoids repeated outline rasterization, but Vello CPU text
redraw still allocates. Scened retains the completed desktop raster so these
allocations stay outside unchanged frames.

The box-blur microbenchmark on the same machine uses ten warm-up iterations
and 200 samples, with no fixture VM running during sampling. It reports only
in-place filtering time; scene composition, backdrop invalidation, IPC and
presentation are excluded. Each pass uses exact reciprocal division with a
remainder correction instead of per-channel variable division.

| CPU blur region | Radius | p50 | p99 | Allocations in 200 iterations |
| --- | ---: | ---: | ---: | ---: |
| 640×480 | 16 | 0.843 ms | 0.894 ms | 0 |
| 64×64 partial output | 16 | 0.0160 ms | 0.0165 ms | 0 |
| 1920×1080 | 32 | 5.996 ms | 6.195 ms | 0 |

Fullscreen CPU blur alone exceeds the 3 ms CPU target on this host. These
measurements do not establish complete compositor or hardware GPU performance.

These measurements exclude the service/driver work listed above and do not
establish 120 Hz presentation or hardware acceptance. All 23 focused host and
product-configuration targets passed, including Flatland geometry/layout/text/
style/input, graphics/runtime, userspace/libc, scened, both VirtIO drivers,
appd, kernel core, boot, migration and splash coverage.

The final graphical guest runs on 2026-09-07 passed with incremental geometry,
desktop text and explicit presentation fences:

| QEMU target under `//testing/e2e/qemu/graphics` | Result |
| --- | --- |
| `boot_ui_aarch64` | Passed, 164.2 s |
| `boot_ui_secure_aarch64` | Passed, 193.3 s |
| `boot_ui_x86_64` | Passed, 139.1 s |
| `input_aarch64` | Passed, 240.0 s |
| `input_x86_64` | Passed, 238.7 s |
| `boot_ui_aarch64 --test_env=BOOT_UI_NO_GPU=1` | Passed, 52.3 s |

The three graphical boot runs produced byte-identical final 800×600 RGB desktop
frames, including Arabic and Devanagari text: zero changed channels across
1,440,000 bytes. This compares CPU output across guest architectures; it is
not a CPU/Vulkan reference comparison. The input fixtures validated real
keyboard/mouse/tablet/multitouch routing, focus isolation, replacement with a
held key and active touch, an unsignaled acquire preserved across replacement,
and subsequent release signaling and delivery to the second focused session.
The first keyboard driver instance is replaced; the multitouch driver itself
is not replaced by this fixture. The geometry allocation
regression passed 200 warmed-up updates in eight embedded views containing
800 nodes with zero allocations and one recomputed node per update. Earlier
embedding/accessibility/input transplant runs passed on AArch64 in 222.0 s and
x86_64 in 239.4 s, including actual magnification, grayscale and focus-ring pixels
and preserved capability identities. The IPC allocation test
uses the host error-returning syscall shim and verifies scratch allocation
behavior only; guest tests establish actual delivery.

Nongraphical kernel smoke tests also passed: AArch64 in 55.2 s and development
x86_64 in 51.6 s. Guest fixture processes terminate at the end of each run.
The Vello/wgpu library build passed, and `bazel run @rules_rust//:rustfmt`
and `git diff --check` completed successfully. No guest Vulkan execution is
implied by the renderer library build.

## Pinned Linux/Venus fixture

`bazel test -c opt --config=e2e //testing/e2e/qemu/venus_linux:host` builds a disposable
AArch64 Linux initramfs from 118 SHA-256-pinned Alpine 3.24 package archives in
`testing/e2e/qemu/venus_linux/packages.prototxt`. The kernel is 6.18.49; the
fixture contains QEMU 11.0.3, virglrenderer 1.3.0 and Mesa/Lavapipe 26.1.6.
Bazel verifies and extracts the archives without running package scripts.
The VM has no network. Xvfb and SDL provide a software OpenGL context required
by QEMU while Lavapipe executes Vulkan. The host tool uses installed macOS
QEMU/HVF (or TCG elsewhere); that outer emulator is not part of the package lock.

The updated host fixture passed in 2.7 s: it checked the CPU Vulkan device, submitted
a buffer fill, waited for its fence, verified every word through coherent
memory readback, and created a QEMU device with Venus/blob enabled. All
QEMU/Xvfb processes are reaped in failure and success paths. This demonstrates
actual Vulkan queue execution inside Linux. The `:guest_aarch64` transport and
scanout test passed in 135.4 s: BexOS created Venus contexts, mapped host-visible
blobs, rejected premature release, queried Vulkan 1.4.347 through the context,
read its reply after a CPU timeline fence, and released the resource. The
displayed CPU scene matched the same pixel samples as native QEMU. That result
preceded the integrated Mesa/wgpu renderer; current guest GPU submission,
readback, effects, text, and worker lifecycle results are recorded above.

The previous QEMU 10.1.5/virglrenderer 1.1.1 pins consistently timed out on
`RESOURCE_UNMAP_BLOB`; single-thread TCG and instruction counting did not fix
that failure. The new releases provide a fixed-address host-memory mapping
path and pass release without changing the driver's deadline or safety checks.

The GL scanout lacks a QMP CPU `DisplaySurface`, so the fixture reads Xvfb's
XWD framebuffer. Since Xvfb has no window manager, the fixture explicitly
moves and resizes the SDL window to 800×600 before comparing guest coordinates.
TrueColor and DirectColor decoding have separate host tests.

The expanded test now packages the existing Rust transplant verifier as a
static Linux executable through an isolated Bazel musl toolchain. Fixture
record version 7 retains a Venus context, resource, mapping and last fence;
older versions 1–6 remain importable. It replaces the GPU/input drivers,
compositor and client with a held key, active touches, a reserved-edge gesture
and an unsignaled presentation acquire. The expanded guest test passed in
280.6 s, including the retained context query, mapping/fence checks, gesture
completion, key/touch release, focus isolation and retired-buffer notification.
Its migration-codec host tests pass. QEMU logs the UART directly and accepts
the verifier on its socket; no serial-forwarding process is needed.

An earlier run failed before takeover. Splash handover now preserves the actual
snapshot/transfer error instead of mapping every failure to `ErrInvalidArgs`.
The passing run did not reproduce that earlier failure, so its cause is not
yet established.

## Earlier Vulkan milestones

The following results describe earlier revisions. Current integration status and
outstanding validation are listed at the beginning of this document.

The Mesa platform adapter and explicit Vulkan entry points now link into the
AArch64 BexOS fixture. Its first nested guest run failed at graphics readiness:
splash presentation timed out and scened did not become ready, leaving appd
unable to supply the fixture's scene grants. Native AArch64 reproduced this
failure (73.1 s), before the fixture received startup. With timing diagnostics,
native AArch64 subsequently passed the full
input/transplant suite in 314.6 s. Input discovery took 3,986 us, display
connection completed by 33,512 us, and desktop initialization by 367,114 us.
GPU/input/scened/client cutovers were 36/51/93/55 ms respectively. All four
kernel/GPU/libc host suites passed. The earlier startup timeouts did not
reproduce in that run, and their cause remains unresolved.
The nested Vulkan probe passed an isolated host-blob mapping, release and context
destruction check, then created real Mesa Vulkan instance and device objects.
It allocated and mapped a 4 KiB buffer, submitted a Vulkan fill command, waited
for its Vulkan fence, and verified all 1,024 words through the guest mapping.
Device-memory teardown initially timed out on blob unmapping (`0x209`).
UNMAP_BLOB now uses a CPU control acknowledgement: host aperture removal does
not require a fence on the unrelated GL timeline. Vulkan submissions and display
operations still require actual GPU completion fences. The updated nested test
passed in 312.0 s, including complete Mesa resource teardown, CPU scanout,
retained Venus resources across replacement, active input and all four service
replacements. GPU/input/scened/client cutovers were 23/54/117/51 ms. This
established guest Vulkan buffer execution; that revision did not yet include
Vello composition or renderer recovery. It did not establish hardware performance.

The C runtime now provides bounded standard-stream formatting through pinned
nanoprintf, checked integer parsing, exit callbacks and a `fstat` alias to the
existing metadata implementation. Guest ABI and allocation probes exercise the
formatting path; native AArch64 passed formatting, integer conversion, metadata
error handling, 16-byte allocation alignment and condition/deadline checks.
The equivalent current x86_64 input/transplant run also passed in 356.6 s.

The C thread runtime now honors stack-size attributes (16 KiB–16 MiB), reports
actual stack bounds, and matches startup records to the kernel-selected stack
instead of assuming worker execution order. The initial thread reserves ID 1;
worker IDs begin at 2. Attribute storage follows the pinned GNU ABI: 64 bytes
on AArch64, 56 bytes on x86_64. Unsupported guard allocation returns an error.
The AArch64 C probe verified a requested 512 KiB worker stack and the address
of a local variable within its reported bounds. Host tests cover out-of-order
worker startup, attribute bounds and the libm entry points used by shader
translation. Joining now claims a single joiner and sleeps on the kernel thread
termination signal, preserving stack/TLS until termination. Invalid, detached,
duplicate and self joins are rejected. The kernel wakes termination waiters and
keeps processes alive while any sibling is blocked. All 126 kernel core tests
and both 12-test libc suites passed. The nested AArch64 guest also passed C
mutex/once, condition, stack-attribute and checked-join probes.

The first nested Vello runs exposed a host Lavapipe worker stack overflow in
LLVM 22's AArch64 assembly finalizer. The faulting stack probe reserves roughly
500 KiB, exceeding musl's default worker stack. The Bazel image builder now sets
the render server's nonexecutable `PT_GNU_STACK` reservation to 8 MiB; only that
ELF metadata changes, and pinned APK inputs remain intact. A host test verifies
the change and rejects malformed headers. Mesa's normal watchdog remains active.
The fixture captures fatal host worker signals and code frames for diagnosis.

With this fix, guest Vello creates its pipelines and texture, submits compute
rendering and readback, verifies background and rounded-rectangle pixels within
one channel value, and releases all Mesa transport leases. The full updated
Linux fixture passed in 312.8 s, including CPU scanout and active-input replacement.
GPU/input/scened/client cutovers were 30/50/71/58 ms. This is functional software
Vulkan evidence, not hardware performance acceptance.

The shared Dual Kawase implementation supports one to five pyramid levels,
retains textures/bindings, and derives each pass's sample bounds backward from
requested output damage. A forward mapping supplies conservative damage after
source updates. The scalar reference checks odd dimensions and image edges;
GPU full and partial updates match it within three unorm channel values, and
pixels outside the requested output remain bit-identical. The full nested
input/replacement run with these blur checks passed in 334.9 s, with GPU/input/
scened/client cutovers of 46/50/66/49 ms. All 11 focused host targets also passed,
including source-damage propagation, graphics/runtime, scened and VirtIO-GPU.
That revision exercised the blur probe independently. Client effects, the service
worker, layout/style/text integration, scanout import, and dynamic input handling
have since been added; their current validation limits are recorded above.

Hardware acceptance still requires a documented GPU-equipped Linux machine,
p99 compositor CPU work within 3 ms, p99 frame processing within 7.5 ms, and
fewer than 1% missed 120 Hz deadlines after warm-up. Neither host library tests
nor software-Vulkan execution would establish those hardware results.
