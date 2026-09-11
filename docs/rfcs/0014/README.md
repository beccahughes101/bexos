# RFC 0014: Driver discovery and lifecycle

- Created: 2026-08-26T21:43:23-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

Appd matches hardware against a driver catalog and activates drivers on demand. Hardware-specific logic stays in D1 drivers, with capability-scoped resources and explicit recovery responsibilities.

## Design overview

`appd` / Driver Manager matches hardware and activates drivers on demand. **Driver Manifest Bind Rules** separate the driver catalog from running driver processes, so boot does not start every installed driver unconditionally.

Related component-based driver models include Fuchsia’s Driver Framework / `driver_manager` and macOS I/O Kit.

## Current Implementation

`appd` now has a host-testable driver manager and lifecycle model:

* App manifests include `driver_info` and repeated `bind_rules` in
  `idl/bexos/app/manifest.proto`; package manifests remain prototxt sources and
  are compiled by Bazel.
* `services/appd/src/driver_manager.rs` builds an in-memory driver index
  from decoded manifests, matches generic device properties against bind rules,
  returns ranked candidates, ranks vendor/device matches above class-level
  matches, applies per-node recovery exclusions and service-contract
  compatibility checks, and applies `DriverPolicy` before allowing a bind
  candidate.
* `DeviceRegistry` tracks device nodes through `Unbound`, `Binding`, `Active`,
  `Quiescing`, `Suspended`, and bind-failure states, validates parent
  registration and caller ownership, stores typed hardware-resource leases, and
  retains selected driver process, manager-channel, and lifecycle handles once
  active.
* The binding-aware appd wave path starts Wave 0 bus drivers first, then
  binds later-wave driver manifests only after a matching device node has been
  registered. The QEMU NVMe package declares a PCI class/subclass/prog-if bind
  rule and no longer needs unconditional launch semantics.

Current D1 transport uses the generated `bexos.hardware.manager.DeviceRegistry`
contract. PCI root registers every memory BAR as a typed MMIO resource, appd
keeps canonical resource handles and passes rights-reduced duplicates through
startup ABI v8, service publication is per device instance with
`device.node_id` metadata, retained provider endpoints can be replayed to a
replacement driver, and the recovery budget retries a crashed driver once before
fallback rebinding. Generic IOMMU-domain resource handoff is modeled, while real
SMMU/IOMMU programming and D2/WASM driver runtime remain future work.

## The Dynamic Match & Bind Lifecycle

```
[ Hardware Bus Enumeration ]
(PCIe Root Driver scans bus; USB Host Controller detects hotplug)
                     │
                     ▼ Emits Hardware Discovery Event
┌────────────────────────────────────────────────────────────────────────┐
│ Driver Manager / appd                                           │
│ 1. Discovers Device Node: { vendor: 0x10DE, device: 0x2204, class: 03 }│
│ 2. Scans Driver Index (matches against installed .bexapp manifests)    │
│ 3. Finds Best Match: //drivers/gpu:nova_driver                         │
│ 4. Spawns D1 Process on-demand & hands over device MMIO/IRQ VMOs       │
└────────────────────────────────────────────────────────────────────────┘
                     │
                     ▼ Spawns isolated process
┌────────────────────────────────────────────────────────────────────────┐
│ D1 Driver Process (e.g., Nova GPU Driver)                              │
│ • Runs initialization, maps MMIO/DMA, exposes bexos.gpu.Device FIDL    │
└────────────────────────────────────────────────────────────────────────┘

```

## Driver Manifest Bind Rules (`driver_manifest.prototxt`)

Every driver `.bexapp` package includes a bind rule section declaring the exact hardware it knows how to drive:

```protobuf
// Package: //drivers/storage:nvme_d1_driver
driver_info {
  name: "nvme_d1_driver"
  package_id: "bexos.driver.storage.nvme"
  version: "1.2.0"
}

// Bind rules evaluated against enumerated hardware properties
bind_rules {
  // Matches any standard NVMe mass storage controller
  condition {
    bus: BUS_PCI
    property { key: "pci.class" value: 0x01 }     // Mass Storage
    property { key: "pci.subclass" value: 0x08 }  // Non-Volatile Memory
    property { key: "pci.prog_if" value: 0x02 }   // NVM Express
  }
}

```

Or for a vendor-specific USB Wi-Fi dongle or GPU:

```protobuf
// Package: //drivers/net:realtek_rtl8153
driver_info {
  name: "rtl8153_driver"
  package_id: "bexos.driver.net.rtl8153"
}

bind_rules {
  condition {
    bus: BUS_USB
    property { key: "usb.vendor_id" value: 0x0BDA }
    property { key: "usb.product_id" value: 0x8153 }
  }
}

```

## The Device Node Tree & Bus Discovery FIDL

The bus drivers (PCIe, USB, Platform DT) do not spawn drivers themselves. They
build an abstract **Device Topology Tree** and notify the system through
`idl/bexos/hardware/manager.fidl`:

```fidl
library bexos.hardware.manager;

using bexos.kernel;

enum Status : int32 {
    OK = 0,
    ERR_INVALID_HANDLE = -1,
    ERR_ACCESS_DENIED = -2,
    ERR_NO_MEMORY = -3,
    ERR_BUFFER_TOO_SMALL = -4,
    ERR_PEER_CLOSED = -5,
    ERR_TIMED_OUT = -6,
    ERR_ALREADY_EXISTS = -7,
    ERR_INVALID_ARGS = -8,
};

enum BusType : uint8 {
    PCI = 1,
    USB = 2,
    PLATFORM_DT = 3,
};

struct DeviceProperty {
    key string:32;
    value uint32;
};

struct DeviceNodeInfo {
    node_id uint64;
    bus BusType;
    properties vector<DeviceProperty>:32;
};

@discoverable
protocol DeviceRegistry {
    RegisterDeviceNode(resource struct {
        info DeviceNodeInfo;
        mmio_vmo handle:VMO;
        irq_channel handle:CHANNEL;
    }) -> (struct {
        status Status;
    });

    UnregisterDeviceNode(struct {
        node_id uint64;
    }) -> (struct {
        status Status;
    });
};

```

## Step-by-Step Execution Sequence

### Catalog Indexing on Install

When a new driver `.bexapp` is installed onto `/pkg`, `appd` extracts its `bind_rules` and adds them to an in-memory index without running the binary.

### Device Discovery

* On boot, `//drivers/d1/bus/generic/pci:pci_root_bus` walks the PCIe configuration space and calls `RegisterDeviceNode` for every device found.
* If a USB flash drive or Wi-Fi adapter is plugged in at runtime, `xhci_usb_driver` detects the hotplug interrupt and calls `RegisterDeviceNode`.

### Pattern Matching

`appd` queries its bind index against the device node's properties (`pci.vendor_id == 0x10DE && pci.device_id == 0x2204`).

### On-Demand Launch

* `appd` creates a new sandbox process for that specific driver.
* Hands over the device's hardware resources (MMIO register VMO, MSI-X interrupt channel, and IOMMU domain handle).

### Dynamic Teardown (Hot-Unplug)

When a device is detached, the bus driver calls `UnregisterDeviceNode`. `appd` sends a graceful shutdown signal over the driver's control channel, reclaims its memory, and terminates the driver process.

## Key Advantages for BexOS

* **Zero RAM Waste:** A machine with 200 installed drivers only consumes RAM and scheduling slots for the 5–10 devices physically present.
* **Instant Hotplug Support:** Adding USB peripherals, external Thunderbolt eGPUs, or PCIe expansion cards uses the exact same lifecycle mechanism as early boot hardware.
* **Easy Fallbacks & Upgrades:** If a high-performance vendor driver is uninstalled or crashes, `appd` can instantly re-evaluate the bind rules and bind a generic fallback driver (e.g., fallback frame-buffer driver).

* ### Drivers

There will be the following levels of drivers:

1. D0 \- In-kernel rust
2. D1 \- Out of process mojo rust
   1. Linux driver compatibility layer
   2. Networking, etc
3. D2 \- WASM
   1. HID

## Driver Policy Enforcement

Board-specific driver privileges are compiled into
`//device/<target>:platform_config_bin` and decoded by `appd` from
`platform.pcfg`.

The policy decision is package and signer based:

- Tier 1 drivers are explicitly allowlisted in `driver_policy.tier_1_allowlist`.
  They receive `DIRECT` hardware access and may bind IRQs, create uncached MMIO
  VMOs, and allocate contiguous physical DMA VMOs.
- Tier 2 drivers receive `ISOLATED` hardware access only when
  `tier_2_rules.enforce_strict_iommu` is true and raw MMIO is disabled. They
  must use bounce-buffer/IOMMU-mediated paths and sanitized register proxies.
- Unmatched drivers use `REJECT_AND_ISOLATE`; appd must not launch them
  for a hardware bind.

`appd` passes the selected hardware access tier to the kernel when it
creates the driver process. The kernel stores it on the process record and
enforces the final MMIO/DMA/IRQ gates even if a buggy userspace service hands a
driver the wrong handles later.

`appd`, the userspace application, package, and component supervisor, should contain no hardware-specific or device-register logic.

Instead, `appd` acts strictly as the **generic orchestrator and lifecycle engine** for drivers. It treats a driver as an isolated component that consumes hardware capability handles and publishes standard FIDL service endpoints.

## What Logic Belongs in `appd` (Generic Driver Management)

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ `appd` Core Driver Engine                                                  │
├─────────────────────────────────────────────────────────────────────────────┤
│ 1. Device Topology & Bus Graph: Maintains parent-child hardware tree        │
│ 2. Manifest Bind Matcher: Evaluates device properties vs. driver manifests  │
│ 3. Capability Slicing: Minting restricted MMIO VMOs, IRQ & IOMMU handles    │
│ 4. Driver Process Isolation & Sandboxing (VMARs, CEL policies, runtimes)   │
│ 5. Dependency Sequencing: Boot-wave staging & bus lockouts                  │
│ 6. Crash Recovery & Heart Transplants: Driver restart and channel rebinding │
└─────────────────────────────────────────────────────────────────────────────┘

```

## The Device Topology Tree

`appd` maintains an in-memory directed acyclic graph (DAG) representing the system's discovered hardware topology:

* **Bus Nodes:** Root Platform Bus $\to$ PCIe Root Controller $\to$ PCIe Bridge $\to$ NVMe Controller.
* **State Tracking:** Tracks whether a device node is `Unbound`, `Binding`, `Active`, `Quiescing`, or `Suspended`.
* **Hotplug & Cascading Teardown:** When a parent bus driver reports that a bridge or USB root hub was detached, `appd` automatically walks down the child tree, sending graceful shutdown signals to all downstream child drivers.

## Manifest Bind Engine

`appd` indexes the `bind_rules` declared by all installed driver packages. When a bus driver registers a new device node with a set of properties, `appd` runs the matching algorithm:

* Computes priority scores based on exact matches:
* Generic Class match (`PCI Class 0x01: Mass Storage` $\to$ Generic Driver: Score 10)
* Vendor + Device ID match (`Vendor 0x10DE, Device 0x2204` $\to$ Nova GPU Driver: Score 100)

* Enforces the **Platform Configuration Policy** (e.g., rejecting third-party drivers if only signed Tier-1 drivers are allowed).

## Hardware Capability Slicing & Delegation

Drivers cannot issue ambient syscalls to map arbitrary physical memory or hook any interrupt. `appd` arbitrates and carves out precise capabilities:

* **MMIO Slicing:** Translates a device's Base Address Registers (BARs) into a restricted physical VMO covering **only that device’s register range**, then maps it into the driver's sub-VMAR.
* **Interrupt Routing:** Obtains an interrupt capability handle from the microkernel for the device's assigned MSI-X vector / IRQ line and passes the channel endpoint into the driver's startup context.
* **IOMMU Domain Assignment:** Configures the hardware IOMMU/SMMU so the driver’s DMA allocations are restricted strictly to physical RAM ranges owned by that driver.

## Boot-Wave & Dependency Sequencing

`appd` manages the bootstrap ordering defined by the platform assembly manifest:

* **Wave 0 (Hardware Discovery & Root Buses):** Spawns PCIe Root Bus, ACPI Parser, and Serial/Debug drivers.
* **Wave 1 (Storage & Base Storage Stack):** Spawns NVMe/VirtIO-Blk drivers, followed by `BexFS` / `BlobFS` services.
* **Wave 2 (Networking & Compute):** Spawns Ethernet/Wi-Fi NIC drivers, GPU services, and netstack.
* **Wave 3 (Peripherals & UI):** Spawns USB host controllers, HID touch/keyboard drivers, audio drivers, and the display compositor.

`appd` blocks higher-wave drivers until prerequisite bus protocols signal readiness over FIDL.

## Crash Resilience & Channel Rebinding

Because D1 drivers run in userspace, hardware drivers can crash or panic without crashing the OS. `appd` manages the recovery cycle:

1. **Failure Detection:** `appd` detects process termination via the driver's task handle.
2. **Channel Stalling:** `appd` signals consuming clients (e.g., VFS or Compositor) that the underlying driver channel is temporarily stalled.
3. **Driver Relaunch:** `appd` respawns the driver process in a clean sandbox, re-hands over the MMIO VMO and IRQ handles, and transfers the existing client communication endpoints.
4. **State Rehydration:** If the driver supports Heart Transplant state serialization, `appd` passes the preserved hardware state VMO to the new instance, resuming operations.

## What Should NEVER Be in `appd`

| Feature | Where It Belongs | Why Keep It Out of `appd` |
| --- | --- | --- |
| **PCIe BAR Parsing / ECAM Math** | D1 PCI Root Bus Driver | Register layout parsing and quirk workarounds should be isolated. |
| **Firmware Parsing & Loading** | Specific D1 Driver | Parsing ELF/binary firmware blobs belongs in the driver component. |
| **Hardware Register Probing** | Specific D1 Driver | Writing bits to hardware registers belongs inside the sandboxed driver. |
| **I/O Protocols (NVMe queues, VirtIO rings)** | Specific D1 Driver | Complex ring buffer protocol logic should never live inside the supervisor. |
