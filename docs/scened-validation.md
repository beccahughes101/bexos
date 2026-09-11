# Scened repository validation

The repository completion boundary is functional/accounting checks and complete
sustained measurement windows. CPU fallback budget misses are measured and
reported. Hardware VSYNC, zero-copy GPU composition, hardware 1080p/120 Hz
acceptance, and the future accessibility architecture remain outside this boundary.

## Reproduction

All builds and protobuf generation use Bazel. Sustained tests are a separate
maintained `//testing/e2e/qemu:performance` matrix, leaving the ordinary graphical
boot/input matrix intact. Run native and nested measurements serially; run host
benchmarks only after all VMs and helpers have exited.

```sh
bazel test -c opt --config=e2e //testing/e2e/qemu/graphics:performance_aarch64 //testing/e2e/qemu/graphics:performance_x86_64 --test_timeout=46000
bazel test -c opt --config=e2e --define=scened_profile_gpu=true //testing/e2e/qemu/venus_linux:performance_all_aarch64 --test_timeout=46000
bazel test -c opt --config=e2e //testing/e2e/qemu/graphics:boot_ui_aarch64 //testing/e2e/qemu/graphics:boot_ui_secure_aarch64 //testing/e2e/qemu/graphics:boot_ui_x86_64 //testing/e2e/qemu/graphics:boot_ui_1080_aarch64 //testing/e2e/qemu/graphics:boot_ui_1080_x86_64 //testing/e2e/qemu/kernel:kernel_smoke_test_aarch64 //testing/e2e/qemu/kernel:kernel_development_smoke_test_x86_64
bazel test -c opt --build_tests_only //kernel/core/... //lib/bexos_libc/... //lib/userspace/... //lib/flatland/... //lib/flatland_cpu/... //lib/flatland_input/... //lib/flatland_layout/... //lib/flatland_style/... //lib/flatland_text/... //lib/flatland_render/... //lib/graphics/... //lib/graphics_runtime/... //lib/venus_transport/... //lib/venus_wgpu/... //lib/virtio_gpu_protocol/... //services/scened/... //services/appd:appd_tests //drivers/d1/display/virtio/gpu:tests //drivers/d1/input/virtio:tests //drivers/d1/bus/generic/pci:pci_root_bus_tests //drivers/d1/bus/generic/pci:runtime_tests //testing/performance/scened:telemetry_tests //testing/e2e/qemu/venus_linux:framebuffer_tests //testing/e2e/qemu/venus_linux:elf_stack_tests //testing/e2e/qemu/graphics:input_hotplug_transport //testing/e2e/qemu:matrix_coverage_test //:heart_transplant_coverage_test
bazel run @rules_rust//:rustfmt
bazel run //testing/e2e/qemu:check_matrix
bazel run -c opt //testing/performance/scened:benchmark -- --output docs/scened-completion-2026-09-08.json --guest-log /tmp/scened-cpu-aarch64-final.log --guest-log /tmp/scened-cpu-x86-final.log --guest-log /tmp/scened-gpu-final.log
```

The nested aggregate performs its existing rendering/input/transplant verification
once, then runs all five sustained workloads with a fresh deadline for each.
`--define=scened_profile_gpu=true` selects a profiled desktop prototxt through
Bazel; normal packages keep profiling disabled. Supported GPU queue timestamps
remain distinct from shader busy time and physical display timing. Its supported
fixture extent is 800×600; the retained host microbenchmarks use 1920×1080.

A local Trusty firmware cache is required before secure graphical boot tests. If
`//third_party/trusty:cached_firmware` reports an incomplete bundle, refresh it
with `bazel run //third_party/trusty:refresh_image`; the refreshed image is a
local gitignored cache artifact, not a generated source file to commit.

## Measurement fields

Each guest window includes epoch/window/workload identity, 32 warmups, 256 measured
completions, CPU and GPU sample counts, CPU nanoseconds, active-loop wall
microseconds, frame dispatch-to-driver-completion latency, timer misses, render
failures, allocation traffic/failures, rendering-path counts and CPU output/GPU
readback bytes. Missing measurements remain null. A successful software fixture
never sets hardware performance acceptance. Guest CPU counters use the guest
scheduler's monotonic clock; they are not QEMU host-thread CPU measurements.
Workload 5 measures imported presentation and retirement; the host scanout
microbenchmark measures only eligibility decisions.

The consolidated report is `docs/scened-completion-2026-09-08.json`. It
uses schema 3, started at `2026-09-08T20:17:20.503111+00:00`, records base
revision `41bd94af48d560b386bf085f95fd50d8249f147a` with the dirty tree used for
this implementation, and ran on an arm64 macOS 26.5.2 host with an Apple M2,
eight logical CPUs, and 8 GiB of memory. Hardware performance verification is
`false` for this record.

## Validation outcome (2026-09-08)

The focused host regression selection passed **45/45 Bazel targets**, including
kernel scheduled CPU/snapshot tests, Rust/C allocator semantics, compositor
metrics/damage/migration, shared rendering/input/layout/style/text, appd/driver
regressions, report collection, image-atlas pressure, and matrix membership. The
latest selection executed four targets and reused 41 successful Bazel results;
preceding selections freshly executed the changed kernel/accounting/metrics
targets, including 135 kernel tests. Bazel Rust formatting, repository-wide matrix
inventory, and heart-transplant manifest/archive coverage also passed.

The boot/kernel gate passed **7/7 targets** after refreshing the local Trusty
firmware cache: standard AArch64 and x86_64 graphical boot, secure AArch64
boot, 1080p graphical boot on both architectures, AArch64 kernel smoke, and x86_64
development kernel smoke. The final run executed six tests, reused one cached
result, and completed in 712.604 seconds.

The native sustained aggregate passed on AArch64 and x86_64. Each run included
the existing input/transplant verifier, completed all five workload windows, used
32 warmups plus 256 measured completions per workload, passed rendering-path,
pixel/progress and retirement checks, and reported zero allocation failures. Both
VM wrappers terminated and reaped their fixture processes.

| AArch64 CPU workload | Samples | Process CPU p99, ms | Active-loop wall p99, ms | Allocation calls | Reallocations | Requested bytes | Output bytes | Timer misses |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Fullscreen | 256 | 39.965 | 50.146 | 25,860 | 512 | 472,406,560 | 491,520,000 | 256 |
| Animated overlap | 256 | 61.350 | 72.694 | 31,231 | 7,424 | 310,303,224 | 146,339,840 | 256 |
| Partial damage | 256 | 29.732 | 30.144 | 24,319 | 2,048 | 305,195,512 | 4,259,840 | 256 |
| Backdrop blur | 256 | 139.747 | 220.888 | 29,954 | 7,424 | 306,287,120 | 172,652,544 | 256 |
| Leased scanout | 256 | 51.708 | 52.016 | 30,208 | 1,280 | 539,733,504 | 0 | 256 |

| x86_64 CPU workload | Samples | Process CPU p99, ms | Active-loop wall p99, ms | Allocation calls | Reallocations | Requested bytes | Output bytes | Timer misses |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Fullscreen | 256 | 116.105 | 185.130 | 26,625 | 512 | 472,412,680 | 491,520,000 | 256 |
| Animated overlap | 256 | 105.436 | 138.112 | 31,997 | 7,424 | 310,309,352 | 146,339,840 | 256 |
| Partial damage | 256 | 40.045 | 44.072 | 24,320 | 2,048 | 305,195,520 | 4,259,840 | 256 |
| Backdrop blur | 256 | 194.172 | 257.071 | 30,461 | 7,424 | 306,291,176 | 172,652,544 | 256 |
| Leased scanout | 256 | 364.711 | 339.030 | 30,214 | 1,281 | 539,733,680 | 0 | 256 |

Every native CPU-composition window missed the 3 ms process CPU and 7.5 ms frame
processing targets and recorded 256/256 timer deadline misses. These TCG results
include real scened sessions, IPC, style/layout preparation, input routing,
composition and completion handling; they are reported misses, not required
optimization work or hardware acceptance.

The nested Venus aggregate passed in 8,784.2 seconds. Workloads 1-4 completed
through the GPU path, each with 256 GPU frames, 256 worker CPU samples, no CPU
fallback output bytes, correct representative pixels, and zero render/allocation
failures. Workload 5 completed 256 direct imported presentations with all releases
retired.

| Nested Venus workload | Samples | Process CPU p99, ms | Active-loop wall p99, ms | Worker CPU p99, ms | GPU frames | Direct frames | GPU readback bytes | Allocation calls | Requested bytes | Timer misses |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Fullscreen | 256 | 3,620.097 | 3,190.553 | 470.785 | 256 | 0 | 491,520,000 | 267,020 | 1,013,408,664 | 256 |
| Animated overlap | 256 | 1,430.224 | 1,102.535 | 331.652 | 256 | 0 | 146,339,840 | 268,110 | 361,575,464 | 256 |
| Partial damage | 256 | 2,342.117 | 1,812.520 | 389.963 | 256 | 0 | 4,259,840 | 261,390 | 356,364,768 | 256 |
| Backdrop blur | 256 | 2,534.227 | 1,921.925 | 665.296 | 256 | 0 | 491,520,000 | 540,468 | 411,202,400 | 256 |
| Leased scanout | 256 | 121.402 | 80.274 | unavailable | 0 | 256 | 0 | 30,208 | 539,733,504 | 256 |

The first nested Venus attempt failed at the partial-damage transition when a 64
MiB allocation exceeded the 256 MiB aperture while 216,170,496 bytes remained
mapped. Scened recovered by falling back to CPU composition, and the strict GPU
fixture correctly rejected the zero-GPU-sample window. Vello atlas pressure now
reclaims unused previous-scene images before growing while retaining images
resolved by the current scene; focused atlas tests and the final nested rerun pass.

The optimized host benchmark collector ran after the VM fixtures exited and wrote
the consolidated report. The 1920×1080 CPU fullscreen benchmark measured p50
0.779 ms and p99 0.817 ms, partial damage p50 0.0129 ms and p99 0.0130 ms,
backdrop fullscreen p50 6.097 ms and p99 6.236 ms, and scanout-eligibility p50
0.000 ms and p99 0.000042 ms. These host microbenchmarks retain their original
scope and do not establish hardware VSYNC or physical display latency.
