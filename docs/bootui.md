# Graphical boot UI

The workstation product includes `splashd`, the CPU `scened` compositor, and D1
VirtIO-GPU and VirtIO input drivers. The nongraphical product does not package these components.
Appd coordinates startup; there is no separate bootsvc process.

## Pipeline

PCI discovery binds the GPU in wave 1. Splashd starts from BootFS in wave 1,
before encrypted storage mounting. Scened starts in wave 5 and waits for appd's
actual storage and service readiness milestones before attempting takeover.
Graphics startup failures are nonessential to the normal boot path.

The GPU driver uses the existing VirtIO PCI transport and HAL with retained DMA
command buffers and two 800×600 BGRA resources. Commands carry fences and have
bounded completion waits. Completed command responses are not hardware VSYNC.
Presentation authority belongs to one granted endpoint and generation. Client
VMOs are validated and copied into driver-owned backing before submission.

`lib/graphics` provides checked surface geometry, format conversion, damage
rectangles, monotonic progress, animation pacing, Flatland graph transactions,
and Vello CPU rendering. `vello_cpu` 0.2.0 is pinned through Bazel, with `libm`
and `u8_pipeline`; no fonts, PNG decoder, Stylo, or Taffy are needed for the
embedded vector wordmark, spinner, and progress bar. Render buffers are reused.
Splash targets 60 Hz; scened targets a 120 Hz timer fallback. Missed deadlines skip frames instead of replaying
an accumulated animation queue.

Splash and compositor create deadline profiles using their own manifest and
platform grants; appd attaches the transferred profiles to their managed threads.
Both request 3 ms capacity. Splash uses a 16.667 ms period; scened requests
a 7.5 ms deadline and 8.333 ms period. Exhausted scheduler budgets
wait for the next period, leaving time for ordinary boot services. RPC waits
block on response channels, and frame waits include migration and client
endpoints. These are scheduling budgets, not a claim of measured 60 FPS.

## Interfaces and ownership

Graphics FIDL bindings are generated through `//idl:graphics_fidl_rust`.
Service discovery names are:

- `bexos.hardware.display.DisplayCoordinator`: display geometry, ownership,
  presentation, frozen-frame snapshots, transfer, and rollback. Private service;
  appd explicitly grants its endpoints to the boot UI services.
- `bexos.splash.ProgressTracker`: bounded progress reports and acknowledged
  compositor handoff. Appd reports real milestones through a dedicated endpoint.
- `bexos.ui.scened.FlatlandSession`: per-endpoint transform trees, root/child
  relationships, translation, positive scaling, clips, opacity, VMO content,
  and queued atomic presentation. At most 16 sessions, 256 transforms per
  session (4096 total), and 512 retained content mappings (256 MiB total)
  are admitted. Migration splits sessions and resources across bounded records.
  Pixels are premultiplied BGRA8 or RGBA8. See [current scened](scened.md) for
  the extended interface, performance changes, and remaining integration work.

Scened imports the frozen splash snapshot and presents identical pixels before
acknowledging takeover. Splashd checks the display generation and completed
ownership transition, releases its mappings, and exits. Scened cross-fades to
its ready background over 250 ms, then presents cached multilingual desktop
text shaped with Parley, laid out with Taffy, and rasterized by Vello CPU.
The text, color, size and insets come from a Bazel-compiled prototxt source. An incomplete transfer expires; the splash
only resumes writes after confirming that ownership has returned. A late
acknowledgement cannot authorize a write or complete a different transfer.

## Firmware framebuffer handoff

Boot handoff version 5 carries optional address, length, dimensions, stride, and
pixel format metadata after the secure-monitor ABI fields. The kernel accepts
the earlier version-4 graphics and secure-monitor layouts and clears extension
fields absent from older handoffs. Version-3 boot-evidence validation is retained.
Existing Multiboot RGB tags and simple-framebuffer DT nodes can supply graphics
metadata. Unsupported or malformed graphics metadata is ignored.
Image assembly emits version 3 for ordinary images without a framebuffer,
preserving compatibility with cached signed BL33. Secure-monitor images and
boot entry adapters that discover framebuffer metadata use version 5.
The kernel reserves the framebuffer range and delegates a non-executable VMO
and descriptor to appd. Rasterization stays in userspace.

Validated graphics boots cover secure/development ARM and development x86.
Current QEMU workstation boot creates its first scanout through VirtIO-GPU;
it does not acquire a firmware GOP framebuffer. The separate integrated x86 product uses the EFI/SVM boot chain described in
[x86 support](x86_64-support.md). Without a working display driver, a supplied firmware framebuffer
can retain splash output while the rest of boot continues.

## Heart transplant

All four components declare `HEART_TRANSPLANT` and provide replacement archives.
Shared migration code retains manager/migration channels, granted endpoint
method restrictions, VMO mappings, and component state. The driver retains DMA
pins, queue counters, scanout resources, fence sequence, and ownership transfer
state. Device submissions complete before a migration polling boundary; adoption
reconstructs the PCI transport without reinitializing the device or scanout.
Renderer scratch state is reconstructed from logical state. Scened prepares
its desktop text cache during bulk transfer, before final quiescence; input
queues, captures, held keys and presentation fence channels survive replacement.

Splash state includes progress, animation timing, and pending handoff. Compositor
state includes both pending and committed scene trees and their retained buffers.
Replacement archives are installed in graphical product update storage.

## Validation

Use Bazel for builds, protocol generation, tests, and formatting:

```
bazel test -c opt //lib/boot:tests //lib/graphics:tests //lib/graphics_runtime:tests //services/splashd:tests //services/scened:tests //drivers/d1/display/virtio/gpu:tests
bazel test --config=e2e //testing/e2e/qemu/graphics:all_architectures
bazel test --config=e2e --define=bootui_transplant_validation=true --test_env=BOOT_UI_VALIDATE_SPLASH=1 //testing/e2e/qemu/graphics:boot_ui_aarch64 //testing/e2e/qemu/graphics:boot_ui_x86_64
bazel test --config=e2e --test_env=BOOT_UI_NO_GPU=1 //testing/e2e/qemu/graphics:boot_ui_aarch64
bazel test //:heart_transplant_coverage_test --test_env=BUILD_WORKSPACE_DIRECTORY="$PWD"
bazel run @rules_rust//:rustfmt
```

The graphical harness saves PPM display captures and serial output as test
artifacts and stops QEMU and its helpers on success or failure. It checks spinner
and progress-bar regions separately, completion fences, splash exit, and
post-transplant presentation. Normal runs replace the GPU and compositor after
takeover. The explicit validation define holds takeover until all three services
have been replaced, allowing a live splash transplant without adding a delay to
production boot. `boot_ui_secure_aarch64` uses authenticated TF-A/Trusty firmware;
the x86 target uses the supported development boot path. Integrated x86 Trusty
is not currently a supported repository boot path.

Host tests cover pixel bounds,
progress ordering, deterministic animation, exact handoff pixels, ownership
rollback, scene isolation, and migration state. Test results and performance
measurements must come from the completed runs; the design's sub-500 KB size
and continuous 60 FPS targets are not assumed guarantees.

## Verification and timing

The 2026-09-06 runs with all three live replacements passed on AArch64
(178.7 s) and x86_64 (191.9 s). Each captured all eight spinner states and five
progress-bar states, verified identical takeover pixels, observed splash exit
with restart disabled, and observed presentation after replacement.

| Guest | Completed splash frames | Mean render + present | GPU / splash / scene cutover |
| --- | ---: | ---: | --- |
| AArch64 | 1,127 | 115.4 ms | 30 / 41 / 37 ms |
| x86_64 | 1,274 | 111.9 ms | 69 / 56 / 41 ms |

Timing uses the guest monotonic clock around rendering, pixel conversion, and
the completed display RPC, excluding the frame-pacing wait. The tests use QEMU
TCG, four virtual CPUs, 1 GiB RAM, and 800×600 scanout on an Apple Silicon host;
host load is uncontrolled. These measurements do not meet uninterrupted 60 FPS.

The final focused host batch passed 16 Bazel targets: boot metadata, shared
graphics and protocol codecs, all three migration-state suites, appd, kernel
core and architecture, libc, BootFS assembly, handoff generation, both product
configurations, and heart-transplant coverage. Failure-path tests cover rejected
candidate state and preservation of the source, pending handoff generations,
ownership timeout, duplicate requests, peer death, and stale completion.
Live tests cover successful replacements; rejected-candidate and pending-handoff
state coverage is at the host-test level.

Additional final checks passed: five x86 host targets (both product configurations,
kernel core, boot metadata, and handoff generation), the shared userspace and
migration suites, authenticated AArch64 graphical boot with normal takeover,
and nongraphical smoke boots on authenticated AArch64 and development x86_64.
The graphical AArch64 image also reached 100% boot readiness without any GPU
device (57.3 s). Normal authenticated graphical boot measured 133.0 ms per
render-and-present operation across 653 splash frames. Normal x86_64 graphical
boot passed in 141.3 s, including GPU/compositor replacements and profile
reattachment, measuring 101.7 ms across 645 splash frames. Its harness waits for
appd's asynchronous profile attachment before ending the test.

## Measured size

Optimized guest ELF measurements on 2026-09-06, in bytes. ELF size includes its
symbol/section metadata; loadable bytes sum `PT_LOAD` file sizes and exclude BSS.

| Component | AArch64 ELF | AArch64 loadable | x86_64 ELF | x86_64 loadable |
| --- | ---: | ---: | ---: | ---: |
| VirtIO-GPU | 999,656 | 450,316 | 963,720 | 499,504 |
| splashd | 1,522,184 | 779,048 | 1,852,360 | 1,191,720 |
| scened | 1,054,160 | 489,356 | 1,045,616 | 567,332 |

The splash does not meet the design's sub-500 KB target. The display currently
uses a fixed 800×600 mode; the renderer itself scales artwork to its surface.
Firmware metadata parsing and reservation have host coverage, while the visible
end-to-end tests exercise VirtIO scanout. Firmware-only output retains the splash;
compositor takeover requires the display service.

The full desktop shell, general input routing, accessibility, GPU rendering,
blur effects, secure unlock overlays, and dynamic layout remain future designs.
See `docs/rfcs/0037/README.md`, `docs/rfcs/0042/README.md`, and `docs/rfcs/0047/README.md`.
