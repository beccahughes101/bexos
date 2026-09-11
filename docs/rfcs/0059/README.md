# RFC 0059: Lazy service activation

- Created: 2026-09-08T08:20:31-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

Appd publishes native service providers before launching them and activates a provider when a declared consumer connects. Discovery stays side-effect free, with explicit idle shutdown and replacement behavior.

## Design overview

Lazy services are native service providers that appd publishes without launching. A provider is activated when a declared consumer asks appd to connect to one of its exposed services. Discovery is side-effect free: listing or resolving the provider does not start the process.

The current rollout implements the broker, startup ABI, shared service-side controller, and keychaind conversion needed for production use. The design below keeps the longer-term shape for additional services, device-scoped providers, and richer service directories, while calling out the current implementation boundaries.

## Manifest contract

`bexos.app.manifest.ExposedService` carries the activation policy for each exposed service while preserving the existing field numbers:

- `activation` defaults to `EAGER`. Eager services keep their existing boot and storage-wave behavior.
- `idle_timeout_ms` is meaningful for lazy services. If it is absent, appd uses 2,000 ms. If it is explicitly zero, the provider becomes idle-eligible as soon as the last tracked client and keep-alive token drops.
- `provider_process` names the service process that owns the exposed protocol. Appd infers it only when the package manifest declares exactly one native service process.

Lazy declarations are accepted only for singleton and user-scoped singleton native service processes with `HEART_TRANSPLANT` lifecycle support. Appd rejects ambiguous provider selection, device-bound services, multiple-instance services, lazy processes without heart transplant support, unspecified activation values, and services sharing one provider process with conflicting activation or idle timeout values. Unsupported lazy shapes fail manifest validation rather than silently running eagerly.

Keychaind is the only production provider marked lazy today. Its manifest is a prototxt manifest and declares the Keychain capability as lazy with `provider_process: "keychaind"`.

## Runtime service directory

Appd exposes an appd-hosted FIDL service-directory interface for userspace runtime discovery and connection. Consumers receive the ServiceDirectory endpoint through an ordinary consumed-service grant and use the userspace helper in `//lib/userspace` to discover and connect to providers at runtime.

A connection request is the activation trigger. The first RPC on the returned channel is not required to start the provider. Before appd creates or forwards any provider endpoint, it authenticates the caller through the appd-issued binding identity and enforces the same policy used for startup grants and optional permission grants:

- the caller must have a matching `services_consumed` declaration;
- the requested provider/capability/method set must fit the provider manifest;
- permission annotations and optional granted values must be satisfied;
- UID-scoped providers must be launched and reused within the caller UID scope;
- locked-user and undeclared requests fail before activation.

Discovery alone never activates a dormant provider.

## Appd lifecycle

Accepted lazy providers are registered explicitly as dormant providers. Dormant registry entries retain the package metadata, archive resources, exposed capability records, and launch information needed after boot. Lazy processes are excluded from automatic BootFS and storage startup waves, but their packages remain available to launch on demand.

Each provider instance is tracked as one of four states:

- `Dormant`: published in the registry, no live process handle.
- `Starting`: appd has accepted one or more pending binds and is launching the provider.
- `Running`: a live provider process owns the manager/control channel.
- `Stopping`: appd has accepted an idle stop request and is waiting for the old process to exit.

Startup grants, runtime ServiceDirectory connects, and optional permission-grant endpoints all flow through the same activation path. Appd reuses singleton providers, isolates user-scoped singleton providers by UID, coalesces concurrent binds while a provider is starting, and detects activation dependency cycles. Pending bind queues are bounded, startup has an explicit timeout, and denied requests, failed launches, uninstall, timeout, and rollback close owned handles and return explicit errors.

Appd creates and retains server endpoints before launch. Initial bindings are delivered in the versioned startup message so the provider can start tracking clients before dispatching them. Later bindings are delivered over the provider manager channel with the same caller metadata and method filters used for startup grants. Existing endpoint recovery behavior is preserved: retained appd endpoints can be replayed to a compatible provider when recovery requires it.

## Startup ABI

The startup ABI is versioned so older launches keep decoding. The current startup message adds lazy-provider metadata and initial incoming bindings while retaining compatibility with older versions:

- structured incoming service binding descriptors and endpoint handles;
- lazy idle timeout and generation metadata;
- lifecycle/control coordination for lazy provider shutdown;
- existing namespace, config, linker, trace, driver, migration, and generic resource fields.

Bazel generates the protobuf and FIDL outputs; generated files are not checked in.

## Idle shutdown

Lazy providers use `//lib/lazy_service` to track demand inside the service process. The library is split into controller, userspace integration, and migration support. It provides connection guards and RAII keep-alive tokens. Providers accept incoming channels through the controller before dispatch, and counts are released when a channel closes or dispatch fails. Manager, migration, and outgoing dependency channels are not counted as clients.

When the final tracked connection and keep-alive token drop, the controller starts a monotonic idle deadline. New connections or tokens cancel stale timers through generation checks. When the deadline fires, the provider sends an idle request containing its generation to appd. Appd rejects stale idle requests. If appd accepts the stop transition, the provider invokes its `prepare_stop` callback, completes service-specific cleanup, closes control channels, and exits. A failed prepare step cancels graceful shutdown and leaves the provider running.

Once appd commits `Stopping`, new binds are queued rather than delivered to the stopping process. After the old process terminates, appd launches one replacement and delivers the queued bindings through the normal activation path. Graceful idle exits are not counted as crash-loop failures and do not trigger automatic eager restart. Unexpected failures still use watchdog/crash-loop handling; lazy providers restart only while outstanding demand exists.

## Heart transplant

Lazy provider state is migratable. During transplant, the service-side controller suspends idle decisions and appd preserves pending bindings, retained handles, lifecycle state, connection ownership, idle generation, keep-alive state, and provider-specific runtime state. Restored providers reconcile the controller snapshot before enabling idle decisions again. If transplant aborts, restored state keeps active clients and pending binds valid and stale idle deadlines are ignored.

Keychaind snapshots now include lazy controller state, guarded live clients, pending idle state, and volatile-store keep-alive state while remaining compatible with supported older snapshots.

## Keychaind rollout

Keychaind no longer starts during the storage service wave. Appd publishes its Keychain service as dormant, launches keychaind on the first declared service connection, and reuses the process for concurrent clients. Final client disconnect starts the configured idle grace period; reconnect after graceful idle exit launches a new keychaind process and reloads persisted secrets.

Keychaind keeps its existing capability filtering and live-client transplant behavior. It accepts both initial startup bindings and later manager-channel bindings through `//lib/lazy_service`, persists and closes vault/session resources in `prepare_stop`, and holds a keep-alive while using volatile storage, including the storage-failure fallback, so idle exit cannot discard secrets.

## Future extensions

The current implementation intentionally rejects lazy device-bound and multiple-instance providers. The long-term design can add device-scoped activation keys, multiple provider instances per capability, richer directory enumeration, provider health hints, demand prioritization, and product policy for lazy activation classes. Those extensions should keep the current invariants: discovery must stay side-effect free, authorization must happen before activation, and providers that can be stopped or replaced must support heart transplant.
