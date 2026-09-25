# D1 Drivers

SDK 0.2 builds native D1 Rust drivers for AArch64 and x86-64. A D1 driver is a
signed service package with exactly one process, at least one bind rule, a
matching `driver_info.package_id`, bounded typed resource requirements, and
heart-transplant support. The current D2 target is only a build smoke target;
there is no supported out-of-tree D2 runtime.

## Bazel target

```starlark
load("@bexos_sdk//rules:defs.bzl", "bexos_driver")

bexos_driver(
    name = "e1000e_aarch64",
    srcs = ["src/main.rs", "src/state.rs"],
    architecture = "aarch64",
    binary_name = "acme_e1000e",
    crate_name = "acme_e1000e",
    manifest = "e1000e.prototxt",
    signing_key = "signing.key",
    std = True,
)
```

Build a separate x86-64 archive with `architecture = "x86_64"`. The product
selects the archive matching its guest architecture.

## Driver manifest

```textproto
package_name: "com.acme.driver.e1000e"
name: "ACME e1000e driver"
min_bexos_abi_version: 1

driver_info {
  name: "acme_e1000e"
  package_id: "com.acme.driver.e1000e"
  version: "1.0.0"
  execution {
    colocation_policy: DRIVER_ISOLATED
    max_instances_per_host: 1
    restart_strategy: DRIVER_HEART_TRANSPLANT
  }
  required_resources { kind: DRIVER_RESOURCE_MMIO min_count: 1 }
  required_resources { kind: DRIVER_RESOURCE_INTERRUPT min_count: 1 }
  required_resources { kind: DRIVER_RESOURCE_IOMMU_DOMAIN min_count: 1 }
  required_resources { kind: DRIVER_RESOURCE_BUS_CONTROL min_count: 1 }
}

bind_rules {
  priority: 200
  condition {
    bus: BUS_PCI
    property { key: "pci.vendor_id" value: 0x8086 }
    property { key: "pci.device_id" value: 0x10d3 }
  }
}

processes {
  name: "acme_e1000e"
  runner: "elf"
  service: true
  wave: 4
  lifecycle {
    update_strategy: HEART_TRANSPLANT
    preparation_timeout_ms: 3000
    migration_timeout_ms: 3000
  }
  runner_options {
    [type.googleapis.com/bexos.app.ELFRunnerOptions] {
      path: "/pkg/bin/acme_e1000e"
    }
  }
}
```

The SDK verifier enforces these bounds:

- `driver_info.name` and `package_id` are nonempty, and the package ID exactly
  matches top-level `package_name`.
- `max_instances_per_host` is between 1 and 64.
- There are between 1 and 16 resource declarations; each uses a known kind and
  requests between 1 and 16 resources.
- Every bind rule has at least one condition; each condition names a supported
  bus and at least one nonempty property key.
- The package contains exactly one service process and at least one bind rule.

Supported bind buses are PCI, USB, platform/DT, I2C, and SPI. Supported resource
kinds are MMIO, interrupt, DMA pool, IOMMU domain, register proxy, and bus
control. Declare the minimum needed by the implementation. A declaration does
not create resources; appd grants only resources registered for the matched
device node.

## Consume structured resources

```rust
#![no_main]

mod state;

use bexos_component::{Channel, Startup};

fn main(startup_channel: u64) -> ! {
    let control = Channel(startup_channel);
    let startup = Startup::receive(control)
        .unwrap_or_else(|_| bexos_component::exit());

    let state = if startup.migration_target {
        bexos_component::live_migration::receive::<state::DriverState>(
            control,
            startup.migration_generation,
        )
        .unwrap_or_else(|_| bexos_component::exit())
    } else {
        if startup.driver_resources.len() < 3 {
            bexos_component::exit();
        }
        let first = startup.driver_resources[0].clone();
        Startup::ready(control).unwrap_or_else(|_| bexos_component::exit());
        state::DriverState::new(control, startup.migration, first.handle)
    };

    state::serve(state)
}

bexos_component::std_entry!(main);
```

Each `StartupHardwareResource` includes `kind`, stable `resource_id`, `base`,
`length`, `flags`, and a rights-reduced `handle`. Match by kind and identity;
do not depend on incidental vector ordering in production code. Use the IOMMU
domain and DMA capabilities supplied by appd instead of raw physical-memory
assumptions.

Drivers may also receive a driver lifecycle channel, host controller, and
recovery handle. Keep those endpoints in the migratable state when the driver
uses them.

## Binding and service publication

Appd evaluates signed bind rules against the registered device topology and
chooses candidates by policy/priority. It preserves canonical resource leases,
passes rights-reduced duplicates to the active driver, and can publish
driver-provided service instances with stable `device.node_id` metadata.

A product must explicitly authorize both native execution and driver execution
for the exact `<package>@<signer>` pair. A valid signature alone cannot make a
package a product driver. See [Product Integration](product-integration.md).

## Driver lifecycle checklist

- Implement bounded drain behavior before quiescence; stop accepting new
  device work while allowing in-flight completions to reach a safe boundary.
- Preserve all live hardware handles, mappings, DMA pins/tokens, queues,
  completion watermarks, client endpoints, and stable request identifiers.
- Make state records architecture-stable and independent of Rust layout or old
  virtual addresses.
- Abort on missing resources, incompatible state versions, unsafe replay, or
  inability to quiesce within the signed timeout.
- Ship and validate a replacement archive, not only an initial driver archive.
