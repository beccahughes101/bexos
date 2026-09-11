# RFC 0013: Power management — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0013](README.md)

## Implementation summary

Powerd implements wake leases, device transition coordination, and provider-backed policy. Hardware execution is delegated to the kernel and depends on the platform.

## Implemented behavior

- PowerPolicy tracks leases and devices, optional telemetry/performance providers, thermal hysteresis, and background CPU throttling. Unavailable measurements are represented explicitly.
- The service coordinates quiescence and rollback around system transitions; kernel power paths handle architecture-specific suspend/reboot/poweroff. AArch64 standby uses PSCI CPU_SUSPEND rather than full system suspend.
- Powerd has a migration adapter preserving leases, providers, clients, and policy state; participating D1 drivers expose power hooks.

## Gaps and deviations

- SUSPEND_TO_DISK explicitly returns unsupported. Full platform suspend/resume, hibernation storage, and physical firmware behavior are not implemented by the QEMU standby path.
- QEMU does not supply a real battery, thermal sensor, or DVFS provider; generic policy does not establish board-level energy savings.
- Transaction tests cover service behavior but do not prove every device can quiesce and resume after real power loss.

## Sources and validation

Implementation and contract evidence: [services/powerd/src/policy.rs](../../../services/powerd/src/policy.rs), [services/powerd/src/service.rs](../../../services/powerd/src/service.rs), [services/powerd/src/migration.rs](../../../services/powerd/src/migration.rs), [kernel/core/src/kernel_services/power.rs](../../../kernel/core/src/kernel_services/power.rs), [kernel/core/src/psci.rs](../../../kernel/core/src/psci.rs), [idl/bexos/power/power.fidl](../../../idl/bexos/power/power.fidl).

Relevant test sources and Bazel targets: [services/powerd/tests/powerd_tests.rs](../../../services/powerd/tests/powerd_tests.rs).

Detailed guides and previously recorded validation: [services](../../services.md), [kernel](../../kernel.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
