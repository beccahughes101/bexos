# Multiarchitecture validation

Results recorded on 2026-09-06 after the multiarchitecture implementation.
Both complete runtime suites passed, followed by final-code checks described below.
These acceptance results predate the subsequent rebase with command/WASM changes.
The merged manifest keeps architecture at field 20 and assigns commands field 21;
kernel snapshots use v16 and debugd snapshots v9 to avoid version collisions.
Rebase validation passed 11 targeted test targets under each architecture selection,
both complete default product builds, and the two boot/handoff test targets.
The Bazel Rust formatter passed. Full runtime suites were not rerun for the rebase.
Large Q35 BootFS images now load after the fixed replacement reservation; the
handoff bounds tests cover both compact and large images and reject RAM overflow.

## Completed checks

| Check | Result |
| --- | --- |
| Q35 development runtime suite | 32/32 passed |
| Kernel, library, service, driver, tool and host tests | 89/89 targets passed under each guest architecture selection |
| Final-kernel ELF/TLS and fault-isolation fixtures | 4/4 passed across ARM and Q35 |
| Kernel and replacement ELF image validation | Passed for both architectures, including optimized images |
| Complete ARM and Q35 development product closures | Built in default and optimized configurations |
| Service/driver replacement-archive coverage | Passed against all 26 heart-transplant manifests |
| Kernel orchestrator model | Passed under both guest architecture selections |
| Bazel Rust formatting and whitespace checks | Passed |
| Complete ARM runtime suite | 28/28 passed |

The initial TLS-pointer bounds check was added during the full acceptance runs.
Both final 89-target host regressions include its rejection/ownership regression:
invalid pointers neither start a thread nor transfer its startup channel.
All four ELF/TLS and fault fixtures subsequently passed with the final kernels.
The full-suite binaries were built before this last input-validation change;
the subsequent host, ELF/TLS, fault and image checks used the final code.

## Runtime evidence

Q35 development boots reach four CPUs, appd, CMOS RTC, PCI/NVMe/BexFS, software
teed, virtio-console, debugd and virtio-net/netstack. The HPET boot check delivers
an external interrupt through IOAPIC and IDT. ELF fixtures verify shared-library
constructors, independent executable/library TLS across three threads, vector
state across timer preemption, and a 32 KiB TCP echo. Fault fixtures isolate
read-only writes, NX execution and unmapped accesses while debug RPC remains usable.

The retained-connection fixture verifies the same TCP connection across network
replacements and the same keychain client and secret across keychaind replacement.

| Guest | virtio-net cutover | netstack cutover | keychaind cutover |
| --- | --- | --- | --- |
| ARM | 50 ms | 35 ms | 33 ms |
| Q35 | 45 ms | 31 ms | 28 ms |

Q35 kernel replacement rejects an invalid candidate, preserves queued requests
and process/registry identities, and verifies allocation from reclaimed old-image
memory. Its observed precommit interval is 2 ms. Kernel takeover retains the
existing CPU0 ownership restriction; active secondary ownership migration remains
outside the supported takeover path.

The complete Q35 suite covers 23 service/driver replacement fixtures, including
appd, storage, networking, CMOS RTC, virtio-console and software teed. Fixtures
check health, registry state and retained client progress after replacement.
The fresh ARM run has passed the formerly failing PCI, combined appd, and VFS
scenarios. Combined appd cutover was 78 ms; BexFS in the VFS prerequisite sequence
cut over in 38 ms. ARM secure teed and RPMB-console fixtures also passed their
rejection/rollback and retained-endpoint checks. The complete ARM suite finished
with all 28 targets passing in 4413.6 seconds; the Q35 suite passed all 32 targets
in 2303.2 seconds. These are whole-invocation timings, including build work.

## Corrections covered by validation

The shared loader preserves TLS zero-fill without overwriting ordinary PT_LOAD
data and handles over-aligned TLS on both architectures. Host tests cover ELF
machine checks, relocations, linking, permissions, malformed input and appd
cleanup/process termination after partial-load failures. Package tests cover
automatic architecture stamping, explicit MULTI, conflicting declarations,
incompatible packages/dependencies and unlabelled native installations.

Service fixes preserve network queues and DMA-domain ownership, keychain state
and client endpoints, and nonpersistent jobs and locked-user state. Timed's DNS
codec/buffer handling and unchanged-state writes were corrected. Appd acknowledges
installation after durable synchronization and drains bounded coordinator progress
batches, including self replacement. Sources retain at most one near-32 KiB record
for the frozen tail, copying the remainder in bounded live catch-up batches.
The 150 ms cutover deadline remains unchanged.

Earlier failed or interrupted invocations are not counted as passing suites.
The current matrix supersedes the initial 29/32 Q35 run and its focused retests.

## Reproduce

Run from the repository root. All builds, fixtures, code generation and tests use Bazel.

```sh
bazel run @rules_rust//:rustfmt
bazel test --config=aarch64 //kernel/... //lib/... //services/... //drivers/... //tools/... //host/...
bazel test --config=x86_64 //kernel/... //lib/... //services/... //drivers/... //tools/... //host/...
bazel test -c opt --config=aarch64 //kernel:image_validation_tests
bazel test -c opt --config=x86_64 //kernel:image_validation_tests
bazel build -c opt //device/virtual/qemu/nongui:virtual_aarch64 //device/virtual/qemu/nongui:virtual_x86_64_development
bazel test //:heart_transplant_coverage_test --test_env=BUILD_WORKSPACE_DIRECTORY="$PWD" --nocache_test_results
bazel test --config=e2e //testing/e2e/qemu:aarch64
bazel test --config=e2e //testing/e2e/qemu:x86_64_development
```

The harness owns QEMU and helper lifetimes and terminates them on completion,
failure or cancellation. The final process audit found no remaining QEMU guests,
RPMB helpers or E2E test processes. The pre-existing modified
`third_party/linux_nvme` working state was left intact.

## Deferred security integration

Q35 development uses explicit software-security fixtures. Integrated x86 Trusty
is unavailable. Secure-only Trusty, protected authentication, rollback and RPMB
acceptance remain separate; development coverage does not satisfy them.
Unsupported secure operations remain unavailable.

Native archives require architecture labels and rebuilding/reinstallation of
old unlabelled native packages. Their signed manifests are never rewritten.
Cross-architecture live migration is excluded.
