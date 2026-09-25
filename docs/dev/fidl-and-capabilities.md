# FIDL And Capabilities

BexOS FIDL defines typed channel protocols; manifests decide whether a process
may provide or consume them. Generating a Rust crate does not grant a route,
and declaring a route does not bypass method, permission, visibility, or signer
policy.

## Generate a local Rust binding

```fidl
library acme.echo;

protocol Echo {
    1: Say(struct { value string:128; })
        -> (struct { value string:128; });
};
```

```starlark
load(
    "@bexos_sdk//rules:defs.bzl",
    "bexos_fidl_rust_library",
    "bexos_service",
)

bexos_fidl_rust_library(
    name = "echo_fidl",
    srcs = ["fidl/echo.fidl"],
    crate_name = "acme_echo_fidl",
)

bexos_service(
    name = "echo_service_aarch64",
    srcs = ["src/main.rs", "src/state.rs"],
    architecture = "aarch64",
    manifest = "echo.prototxt",
    signing_key = "signing.key",
    fidl_deps = [":echo_fidl"],
)
```

The full rule is:

```starlark
bexos_fidl_rust_library(
    name,
    srcs,
    crate_name = None,
    deps = [],
    fidl_deps = [],
    visibility = None,
)
```

`fidl_deps` supplies imported `.fidl` source files to the compiler. `deps`
supplies generated Rust libraries needed by the emitted binding. Generation is
a Bazel action; do not commit the emitted `.rs` file.

Public system bindings are already exported below `@bexos_sdk//idl`. For
example, the SDK contains generated targets for kernel, block, filesystem,
hardware-manager, migration, network, power, preferences, time, trust, user,
and VFS protocols. Prefer those targets to copying platform FIDL into the
external repository.

## Expose a service

```textproto
services_exposed {
  name: "com.acme.echo.Echo"
  protocol: "Echo"
  lifecycle: SINGLETON
  visibility: PUBLIC
  provider_process: "echo_service"
  capabilities {
    capability: "Public"
    method_ordinals: 1
  }
}
```

An exposed declaration identifies the routing name, protocol, lifecycle,
visibility, optional bind permission, capability groups, method ordinals, and
provider process. Valid visibility choices are:

- `PUBLIC`: visible to all otherwise authorized consumers.
- `PRIVATE`: visible only inside the provider package.
- `DOMAIN_SHARED`: visible within the matching package-name domain.
- Unspecified: hidden.

`SINGLETON`, `USER_SCOPED_SINGLETON`, and `MULTIPLE_INSTANCE` select instance
semantics. Drivers commonly publish multiple device instances with
`device.node_id` metadata even when one driver package owns them.

For on-demand startup, add `activation: LAZY` and an optional finite
`idle_timeout_ms`. The provider process must be unambiguous and cannot conflict
with unsupported lifecycle/instance combinations.

## Consume a service

```textproto
services_consumed {
  name: "com.acme.echo.Echo"
  link_type: REQUIRED
  capabilities {
    capability: "Public"
    methods { ordinal: 1 link_type: REQUIRED }
  }
}
```

Appd intersects the consumer's requested capability/method set with the
provider declaration. Required methods must exist or launch/binding fails.
Optional methods are granted only when present. An empty method list requests
no provider methods and does not create a usable service channel.

At native startup, granted routes appear in `Startup.service_grants` and
incoming clients appear in `Startup.incoming_service_grants`. Each
`ServiceGrant` carries the service and protocol names, capability, allowed
method ordinals, granted permission values, caller/provider metadata, and the
endpoint handle. Providers must still enforce the allowed ordinals while
dispatching.

## Permissions

Permissions are structured declarations, not legacy strings:

```textproto
permissions {
  name: "com.acme.echo.use"
  values: "basic"
  requirement: PERMISSION_REQUIRED
  usage_description: "Send text to the local echo service"
}
```

They may be package-wide or process-specific. An exposed capability can bind a
permission name, and platform/user policy decides whether the package receives
it. Declare optional access as `PERMISSION_OPTIONAL`; do not model optional
behavior by pretending required service links can fail silently.

Method-level FIDL `@permission` contracts and manifest capability scopes are
both enforced. Keep ordinals stable once published. Changing or reusing an
ordinal changes the compatibility and authorization contract even when Rust
method names still compile.

## Compatibility rules

- BexOS FIDL is a BexOS dialect, not source- or wire-compatible with Fuchsia
  FIDL. See [the comparison](../fidl-vs-fuchsia.md).
- Generated bindings are `no_std` compatible and use the BexOS channel ABI.
- Published protocol evolution must preserve existing ordinals and bounded
  wire types.
- Provider and consumer manifests must evolve together when capability names
  or required method sets change.
- A replacement service must preserve its published service contract across
  migration; otherwise retained provider endpoints cannot be safely rebound.

For the full current IDL inventory and wire behavior, see
[IDL And ABI](../idl-and-abi.md) and [FIDL permissions](../fidl-permissions.md).
