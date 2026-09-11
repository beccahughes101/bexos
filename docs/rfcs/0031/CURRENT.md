# RFC 0031: Timekeeping and synchronization — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0031](README.md)

## Implementation summary

Kernel time primitives and a read-only time page are integrated with timed, RTC providers, and SNTP/NTS client code.

## Implemented behavior

- The kernel maintains monotonic/boottime and a privileged realtime transform; libc can read the seqlock time page with a syscall fallback.
- Timed implements initial/manual steps, bounded subsequent slew, quality tracking, persisted target state, RTC bootstrap/writeback, and configured network synchronization.
- NTS code handles key-exchange records, cookies, authenticated NTP extensions, and response validation. Timed migration includes synchronization/configuration, cookies, deadlines, and watcher state.

## Gaps and deviations

- Timezone databases, calendar formatting, and leap policy are not a full kernel feature; the WASI environment currently exposes UTC timezone information.
- NTS protocol code and configured bootstrap are not proof of successful interoperability with every public service or deployment trust root.
- The RFC’s fast-read nanosecond estimates are not newly measured here. Physical oscillator drift, suspend clocks, and board RTC behavior require platform-specific evidence.

## Sources and validation

Implementation and contract evidence: [lib/time_abi](../../../lib/time_abi), [lib/bexos_libc](../../../lib/bexos_libc), [kernel/core/src/kernel_services/time.rs](../../../kernel/core/src/kernel_services/time.rs), [services/timed/src/service.rs](../../../services/timed/src/service.rs), [services/timed/src/nts.rs](../../../services/timed/src/nts.rs), [services/timed/src/migration.rs](../../../services/timed/src/migration.rs).

Relevant test sources and Bazel targets: [services/timed/tests/timed_tests.rs](../../../services/timed/tests/timed_tests.rs), [lib/time_abi/BUILD.bazel](../../../lib/time_abi/BUILD.bazel), [kernel/core/BUILD.bazel](../../../kernel/core/BUILD.bazel).

Detailed guides and previously recorded validation: [services](../../services.md), [kernel](../../kernel.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
