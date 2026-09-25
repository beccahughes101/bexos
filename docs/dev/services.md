# Services

Services are long-running providers started and supervised by appd. SDK 0.2
supports native AArch64/x86-64 services and portable WASM services. Every
service process in an SDK package must declare `HEART_TRANSPLANT`; package
creation rejects a service that falls back to restart-only lifecycle.

## Native service target

```starlark
load(
    "@bexos_sdk//rules:defs.bzl",
    "bexos_fidl_rust_library",
    "bexos_service",
)

bexos_fidl_rust_library(
    name = "echo_fidl",
    srcs = ["fidl/echo.fidl"],
)

cc_library(name = "c_helper", srcs = ["src/c_helper.c"])

bexos_service(
    name = "echo_service_aarch64",
    srcs = ["src/main.rs", "src/state.rs"],
    architecture = "aarch64",
    binary_name = "echo_service",
    crate_name = "acme_echo_service",
    manifest = "echo.prototxt",
    signing_key = "signing.key",
    fidl_deps = [":echo_fidl"],
    link_deps = [":c_helper"],
    std = True,
)
```

Create a corresponding x86-64 target by changing the target name and setting
`architecture = "x86_64"`. `std = True` selects the source-distributed BexOS
Rust standard-library/libc runtime. Leave it false for freestanding Rust. C
code is linked through ordinary Bazel `cc_library` targets in `link_deps`.

## Manifest process

```textproto
package_name: "com.acme.echo"
name: "ACME echo service"
min_bexos_abi_version: 1
processes {
  name: "echo_service"
  runner: "elf"
  service: true
  wave: 7
  lifecycle {
    update_strategy: HEART_TRANSPLANT
    preparation_timeout_ms: 30000
    migration_timeout_ms: 1000
  }
  runner_options {
    [type.googleapis.com/bexos.app.ELFRunnerOptions] {
      path: "/pkg/bin/echo_service"
    }
  }
}
```

All processes in a package imported as `component_type = "service"` must be
service processes. If a package has several service processes, their declared
waves must agree. `wave` is required for an early `BOOTFS` import because the
product importer compares the signed wave to its explicit `boot_wave`.

Use a wave only when the product should start the process automatically. A
lazy exposed service may instead identify its `provider_process` and allow appd
to launch it on first bind.

## Structured native startup

Native services receive a startup channel as `_start`'s first argument. Decode
it before announcing readiness:

```rust
#![no_main]

mod state;

use bexos_component::{Channel, Startup};

fn main(startup_channel: u64) -> ! {
    let control = Channel(startup_channel);
    let startup = Startup::receive(control)
        .unwrap_or_else(|_| bexos_component::exit());

    let state = if startup.migration_target {
        bexos_component::live_migration::receive::<state::ServiceState>(
            control,
            startup.migration_generation,
        )
        .unwrap_or_else(|_| bexos_component::exit())
    } else {
        Startup::ready(control).unwrap_or_else(|_| bexos_component::exit());
        state::ServiceState::new(control, startup.migration)
    };

    state::serve(state)
}

bexos_component::std_entry!(main);
```

For a freestanding service use `bexos_component::entry!` instead of
`std_entry!`. The decoded `Startup` also carries namespace directories,
component configuration, generic resources, incoming and consumed service
grants, trace data, locale data, and migration metadata. Treat every handle as
capability-scoped input; do not rediscover resources globally.

Call `Startup::ready` only after the initial instance has accepted its startup
state and can serve. A migration target is activated by the migration protocol,
so it must receive and validate transferred state rather than call the cold
startup path.

## Expose and consume protocols

The manifest is the authority for routing. An exposed service declares its
protocol, lifecycle, visibility, capability/method set, and provider process.
A consumed service declares required or optional capability methods. See
[FIDL And Capabilities](fidl-and-capabilities.md) for a complete example.

Appd will not mint an endpoint merely because a FIDL crate is linked. The
consumer must declare the service, the provider must expose it, visibility and
permission checks must pass, and required method ordinals must exist.

## Portable WASM service

A portable service is packaged with `bexos_wasm_app`, but its manifest process
has `service: true` and `HEART_TRANSPLANT`. Implement the SDK lifecycle world:

```rust
use bexos_wasm_guest::exports::bexos::wasm::lifecycle::Guest;

struct EchoService;

impl Guest for EchoService {
    fn dispatch(_resource_id: u32) {}
    fn checkpoint() -> Vec<u8> { Vec::new() }
    fn restore(_checkpoint: Vec<u8>) -> Result<(), ()> { Ok(()) }
    fn activate() { println!("echo: active"); }
    fn abort() {}
}

fn main() {}

bexos_wasm_guest::export!(EchoService with_types_in bexos_wasm_guest);
```

`checkpoint` must return a bounded, versioned representation of durable live
state; an empty checkpoint is appropriate only for a genuinely stateless
service. `restore` must reject malformed or unsupported state. `activate` is
the commit-side transition and `abort` must leave the old owner authoritative.

## Service checklist

- Every process is `service: true` and declares `HEART_TRANSPLANT`.
- The ELF/WASM path matches the archive entry generated by its Bazel rule.
- Readiness is emitted only after cold-start initialization succeeds.
- Migration targets restore state and do not repeat exclusive initialization.
- Exposed and consumed services are declared in the manifest, including method
  scopes and permissions.
- Boot wave and service dependencies are selected by the product's actual
  startup order, not copied blindly from the fixture.
