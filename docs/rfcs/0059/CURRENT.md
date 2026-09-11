# RFC 0059: Lazy service activation — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0059](README.md)

## Implementation summary

Lazy activation is implemented for supported native service shapes, with keychaind as the only production manifest marked lazy.

## Implemented behavior

- Appd publishes dormant services; discovery does not launch them. Authorized demand drives Dormant/Starting/Running/Stopping state with bounded pending binds and a startup deadline.
- Manifest validation requires an unambiguous native service provider with HEART_TRANSPLANT support and compatible activation/idle settings. Lazy providers skip boot waves.
- Lib/lazy_service tracks clients/keep-alives and coordinates intentional idle shutdown. Checkpoint records preserve provider generations, pending endpoints, idle state, and service controller state.
- Keychaind integrates guarded service bindings, persistent storage, idle behavior, and replacement archives.

## Gaps and deviations

- Device-bound, multiple-instance, non-native, and non-migratable providers are rejected rather than automatically converted to eager services.
- Richer enumeration, health hints, demand priority, and rollout to additional services remain future extensions.
- The 2-second default idle timeout and 5-second startup timeout are policies, not proof that all future providers can start or drain within those bounds.

## Sources and validation

Implementation and contract evidence: [services/appd/src/lazy.rs](../../../services/appd/src/lazy.rs), [services/appd/src/manifest.rs](../../../services/appd/src/manifest.rs), [services/appd/src/waves.rs](../../../services/appd/src/waves.rs), [lib/lazy_service/src](../../../lib/lazy_service/src), [services/keychaind/package/keychaind.prototxt](../../../services/keychaind/package/keychaind.prototxt), [services/keychaind/src/runtime.rs](../../../services/keychaind/src/runtime.rs).

Relevant test sources and Bazel targets: [services/appd/tests/lazy_tests.rs](../../../services/appd/tests/lazy_tests.rs), [lib/lazy_service/BUILD.bazel](../../../lib/lazy_service/BUILD.bazel), [services/keychaind/tests](../../../services/keychaind/tests).

Detailed guides and previously recorded validation: [appd userspace](../../appd-userspace.md), [testing status](../../testing-status.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
