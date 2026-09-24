# RFC-0067: Driver Framework, Dynamic Topology Management, and Sandboxed Host Isolation

* **Author:** BexOS Hardware Architecture & Platform Working Group
* **Status:** Proposed
* **Target Subsystems:** `driver_manager`, `devhost`, `pkgd`, `kernel` (D0), `networkd`, `scened`
* **Applicability:** Bus Enumerators, Peripheral Device Drivers, System Services

> **Implementation selection:** The current implementation is documented in
> [CURRENT.md](CURRENT.md). BexOS currently uses `appd` as the topology and
> driver-lifecycle coordinator and launches each driver host as a native ELF
> process. It does not load arbitrary shared objects into a generic `devhost`.
> The separate `driver_manager`/generic `devhost` architecture below remains the
> long-term design and is intentionally retained here.

---

## 1. Summary

This RFC defines the driver model for BexOS. It introduces:

1. **`driver_manager` (D1 Driver Topology Service):** The centralized coordinator responsible for bus enumeration, driver binding resolution, capability attenuation, and device lifecycle orchestration.
2. **`devhost` (Driver Host Process):** Sandboxed, unprivileged execution containers dynamically instantiated by `driver_manager` to house driver components.
3. **Multi-Instance Isolation Model:** Declarative colocation policies allowing drivers managing multiple hardware endpoints (e.g., multi-NIC controllers, multiple PCIe ports) to run either in fully isolated, process-per-device configurations or grouped in single-process hosts.
4. **Dynamic Driver Acquisition via `pkgd`:** Transparent, on-demand resolution and mounting of signed driver packages from OCI registries based on hardware vendor and product IDs.
5. **Direct Zero-Copy Client Data Plane:** Control path mediated by `driver_manager`, while high-throughput I/O (network rings, display framebuffers, NVMe queues) flows directly between device hosts and consuming system daemons via shared `VMO` rings.

---

## 2. Motivation

Monolithic kernels run all device drivers in supervisor mode (Ring 0 / EL1). A null-pointer dereference, heap corruption, or rogue DMA transaction in a Wi-Fi or Ethernet driver compromises the entire operating system.

Previous microkernel driver models have historically introduced two primary failures:

* **Ambient Internal Process Spawning:** Drivers that detect multiple hardware instances frequently attempt to fork or spawn their own internal child processes. This breaks the microkernel capability boundary because drivers must be granted ambient process-creation and capability-forging rights.
* **Coarse Isolation vs. IPC Overhead:** Forcing every hardware sub-node (e.g., an individual I2C sensor or GPIO pin) into its own process incurs high IPC and context-switching overhead, while colocating all drivers into a single monolithic "driver container" destroys fault isolation: a crash in an audio codec driver takes down the storage controller.

BexOS resolves these trade-offs by centralizing topology decisions within `driver_manager`, enforcing strict hardware capability compartmentalization (IOMMU domains, MMIO ranges, MSI-X vectors), and decoupling device logic from process orchestration.

---

## 3. System Architecture & Topology

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ D0 MICROKERNEL (L0 Hypervisor / Hardware Primitives)                        │
│ • Manages hardware interrupts (MSI-X / GIC) ──► Delivers to `zx.Handle:INTERRUPT`
│ • Programs hardware IOMMU (VT-d / AMD-Vi / SMMU) via `zx.Handle:IOMMU`      │
│ • Restricts physical address ranges via `zx.Handle:RESOURCE`               │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ System Privileges
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ `driver_manager` (D1 Hardware Topology Coordinator)                         │
│                                                                             │
│  [ Bus Topology Graph ]                                                     │
│  • Root Buses: ACPI, Device Tree, PCIe Root Complex, USB, I2C, SPI         │
│  • Matches device signatures (VID/PID/Class) against driver manifests       │
│  • Queries `pkgd` for missing driver artifacts over OCI                     │
│  • Enforces Colocation and Restart Policies                                 │
└──────────┬───────────────────────────────────────┬──────────────────────────┘
           │ Spawns Devhost A (Job A)              │ Spawns Devhost B (Job B)
           ▼                                       ▼
┌─────────────────────────────────────┐ ┌─────────────────────────────────────┐
│ `devhost` (PCIe 0000:01:00.0)       │ │ `devhost` (PCIe 0000:02:00.0)       │
│ [NIC 0 / eth0]                      │ │ [NIC 1 / eth1]                      │
│                                     │ │                                     │
│  [ libintel_e1000e.so ]             │ │  [ libintel_e1000e.so ]             │
│  • MMIO BAR 0 VMO                   │ │  • MMIO BAR 0 VMO                   │
│  • MSI-X Vector Handle              │ │  • MSI-X Vector Handle              │
│  • IOMMU Domain Handle              │ │  • IOMMU Domain Handle              │
└──────────────────┬──────────────────┘ └──────────────────┬──────────────────┘
                   │                                       │
                   │ Direct Packet Ring (VMO)              │ Direct Packet Ring (VMO)
                   ▼                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ `networkd` (D1 Network Protocol Stack)                                      │
│ Binds directly to both interfaces; zero mediation via `driver_manager`      │
└─────────────────────────────────────────────────────────────────────────────┘

```

---

## 4. Multi-Instance Hardware Topology & Multi-NIC Architecture

When managing hardware with multiple physical endpoints (e.g., dual discrete PCIe NICs, or a multi-port NIC with multiple Physical Functions), drivers do **not** spawn child worker processes.

### 4.1 Driver Manifest & Colocation Policies

Each driver package includes a declarative manifest (`driver.json`) specifying hardware match rules, capabilities, and isolation topology:

```json
{
  "driver_name": "intel-e1000e",
  "version": "1.4.0",
  "match_rules": [
    {
      "bus": "pci",
      "vendor_id": "0x8086",
      "device_ids": ["0x10d3", "0x1539", "0x15b8"]
    }
  ],
  "execution": {
    "colocation_policy": "isolated",
    "max_instances_per_host": 1,
    "restart_strategy": "heart_transplant"
  },
  "required_capabilities": [
    "pci_bar_mmio",
    "msix_interrupt",
    "iommu_dma"
  ]
}

```

#### Colocation Modes

| Mode | Topology Description | Use Cases & Fault Domain |
| --- | --- | --- |
| **`isolated` (Default)** | Every physical device node ($BDF$) receives an independent `devhost` process and sub-job sandbox. | High-throughput, high-risk controllers: discrete NICs, NVMe drives, USB host controllers, Wi-Fi radios. |
| **`colocated`** | Multiple matching device nodes are loaded as separate dynamic instances within a single `devhost` process. | Low-overhead, lightweight peripheral devices: I2C environmental sensors, SPI flash chips, multi-UART controllers. |
| **`host_shared`** | The driver component is loaded directly into its parent bus driver's existing `devhost` process. | Performance-critical board-level glue logic: GPIO pin expanders and clock gates directly tied to SoC core registers. |

### 4.2 Multi-NIC Hardware Scenarios

1. **Independent PCIe Cards (e.g., two discrete NICs on separate slots):**
* Bus enumeration discovers `0000:01:00.0` and `0000:02:00.0`.
* `driver_manager` instantiates two distinct `devhost` processes.
* If `devhost-01:00.0` panics, its process terminates. `devhost-02:00.0` continues processing frames uninterrupted.


2. **Multi-Function PCIe Cards (SR-IOV or Physical Functions 01:00.0 and 01:00.1):**
* The PCIe bus enumerator treats each function ($F_0, F_1$) as an independent device node.
* If the manifest sets `colocation_policy: "isolated"`, each physical port executes in a separate process with isolated IOMMU page tables.


3. **Single Controller with Multiple Physical PHYs/MACs:**
* For hardware sharing a unified register bank, the driver registers multiple **child device nodes** (e.g., `eth0`, `eth1`) through `driver_manager`.
* Each child port exposes its own independent FIDL packet ring channel directly to `networkd`.



---

## 5. Capability Attenuation & Hardware Isolation

Drivers in BexOS possess no ambient authority. The microkernel grants physical hardware access exclusively through unforgeable capabilities passed from `driver_manager` during instance binding.

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ MICROKERNEL CAPABILITY BOUNDARY                                             │
│                                                                             │
│  1. MMIO Registers:                                                         │
│     The kernel maps only the explicit physical address bounds of BAR 0 into │
│     a `VMO`. `devhost` maps this VMO via `zx_vmar_map`. It cannot read or   │
│     write adjacent hardware registers or system memory.                     │
│                                                                             │
│  2. DMA & IOMMU:                                                            │
│     `devhost` receives a `zx.Handle:IOMMU` pinned to its specific PCIe      │
│     Requester ID (BDF). Any DMA read/write outside programmed VMO buffers   │
│     triggers an IOMMU Page Fault, instantly halting the rogue transaction.  │
│                                                                             │
│  3. Interrupts:                                                             │
│     Physical IRQs / MSI-X vectors are reflected by the microkernel to       │
│     `zx.Handle:INTERRUPT` primitives. The driver awaits signals via         │
│     `zx_interrupt_wait()` without running arbitrary ISRs in Ring 0.         │
└─────────────────────────────────────────────────────────────────────────────┘

```

---

## 6. Dynamic Driver Resolution via `pkgd`

BexOS base installations do not need to bundle terabytes of vendor driver blobs. When an unknown peripheral is plugged in or enumerated on the bus:

```
Bus Controller            `driver_manager`             `pkgd`              OCI Registry
      │                          │                        │                      │
      │ 1. Device Detected       │                        │                      │
      │    (PCI: 8086:1539)      │                        │                      │
      ├─────────────────────────►│                        │                      │
      │                          │ 2. Check Local Driver  │                      │
      │                          │    Index (Miss)        │                      │
      │                          │                        │                      │
      │                          │ 3. ResolveArtifact()   │                      │
      │                          │    (kind: DRIVER)      │                      │
      │                          ├───────────────────────►│                      │
      │                          │                        │ 4. Fetch & Validate  │
      │                          │                        │    TUF Metadata      │
      │                          │                        ├─────────────────────►│
      │                          │                        │◄─────────────────────┤
      │                          │                        │                      │
      │                          │ 5. Return Read-Only    │                      │
      │                          │    Driver Package VMO  │                      │
      │                          │◄───────────────────────┤                      │
      │                          │                                               │
      │                          │ 6. Spawn `devhost`                            │
      │                          │ 7. Map Driver `.so` & execute `Driver::bind`  │

```

1. **Enumeration:** A bus enumerator (PCIe, USB) reports a device descriptor: Vendor `0x8086`, Device `0x1539`.
2. **Registry Lookup:** If no registered driver matches locally, `driver_manager` dispatches an `ArtifactQuery` to `pkgd`.
3. **TUF Verification:** `pkgd` downloads the OCI artifact (e.g., `pkg.bexos.org/drivers/net/intel-e1000e:1.4.0`), verifies its cryptographic signatures against the root TUF keys, and checks rollback monotonic counters in `trusty` RPMB.
4. **Ingestion & Launch:** `pkgd` returns an immutable, read-only VMO of the driver package. `driver_manager` instantiates a new `devhost`, maps the driver binary, and calls its entrypoint.

---

## 7. Driver Lifecycle & The Heart Transplant Pattern

When a driver crashes, encounters an unrecoverable hardware state, or receives a live software update:

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ DRIVER FAULT & RECOVERY SEQUENCE                                            │
│                                                                             │
│ 1. Crash Detection:                                                         │
│    `devhost` crashes (e.g., panic or fatal page fault). `driver_manager`    │
│    receives a `ZX_CHANNEL_PEER_CLOSED` signal on the host control port.     │
│                                                                             │
│ 2. Hardware Quiescence:                                                     │
│    `driver_manager` asserts PCIe Bus Master Disable (BMD) via PCI root bus  │
│    primitives, terminating all active DMA transactions immediately.         │
│                                                                             │
│ 3. Channel Stashing (Heart Transplant):                                     │
│    `networkd` does not close its data-plane channel. Its FIFO queue pauses. │
│                                                                             │
│ 4. Host Respawn:                                                            │
│    `driver_manager` starts a fresh `devhost` process, re-maps physical      │
│    registers, resets the device engine, and re-attaches existing data-plane │
│    VMO handles.                                                             │
│                                                                             │
│ 5. I/O Resume:                                                              │
│    Packet transmission resumes. The user experiences sub-50ms jitter with   │
│    zero application termination or system reboot.                           │
└─────────────────────────────────────────────────────────────────────────────┘

```

---

## 8. Interface Definition Language (FIDL)

The core driver protocols are defined in `idl/bexos/hardware/driver.fidl`:

```fidl
library bexos.hardware;

using bexos.kernel;

type BusType : uint8 {
    PCI = 1;
    USB = 2;
    I2C = 3;
    SPI = 4;
    PLATFORM = 5;
};

struct DeviceIdentity {
    bus BusType;
    vendor_id uint32;
    device_id uint32;
    subsystem_id uint32;
    revision uint8;
    topological_path string:256; // e.g. "/dev/sys/pci/0000:01:00.0"
};

struct HardwareResources {
    mmio_vmos vector<zx.Handle:VMO>:8;
    interrupts vector<zx.Handle:INTERRUPT>:16;
    iommu_token zx.Handle:IOMMU;
};

@discoverable
protocol DriverHostController {
    /// Instructs a devhost to bind and initialize a driver against assigned hardware
    BindDevice(resource struct {
        identity DeviceIdentity;
        resources HardwareResources;
        driver_binary zx.Handle:VMO;
        device_control_channel zx.Handle:CHANNEL;
    }) -> () error bexos.kernel.Status;

    /// Cleanly unbinds and quiesces hardware prior to shutdown or replacement
    UnbindDevice() -> () error bexos.kernel.Status;
};

@discoverable
protocol NetworkDeviceDataPlane {
    /// Register shared-memory VMO descriptors for zero-copy DMA packet transfer
    RegisterPacketRings(resource struct {
        rx_ring_vmo zx.Handle:VMO;
        tx_ring_vmo zx.Handle:VMO;
        ring_descriptors zx.Handle:VMO;
    }) -> () error bexos.kernel.Status;

    /// Signal that transmission buffers are pending in the shared ring
    NotifyTx();
};

```

---

## 9. Rust Driver SDK API (`libdriver`)

Driver authors implement standard traits without interacting with low-level microkernel syscalls:

```rust
use bexos_driver::prelude::*;

pub struct IntelE1000Driver {
    mmio: MmioRegion,
    irq: InterruptHandle,
    iommu: IommuDomain,
}

impl Driver for IntelE1000Driver {
    type Instance = IntelE1000Instance;

    fn bind(context: DriverBindContext) -> Result<Self::Instance, DriverError> {
        let mmio = context.take_mmio(0)?;
        let irq = context.take_interrupt(0)?;
        let iommu = context.take_iommu()?;

        let mut instance = IntelE1000Instance { mmio, irq, iommu };
        instance.reset_hardware()?;
        instance.init_rx_tx_rings()?;

        Ok(instance)
    }
}

impl DeviceInstance for IntelE1000Instance {
    fn handle_interrupt(&mut self) {
        // Acknowledges hardware interrupt and processes descriptors
        let status = self.mmio.read32(REG_ICR);
        if status & ICR_RXT0 != 0 {
            self.flush_rx_descriptors();
        }
    }

    fn quiesce(&mut self) {
        // Disable interrupts and stop DMA engines
        self.mmio.write32(REG_IMC, 0xFFFFFFFF);
        self.mmio.write32(REG_RCTL, 0);
        self.mmio.write32(REG_TCTL, 0);
    }
}

```

---

## 10. Security Analysis & Threat Matrix

| Threat Model | Kernel / Platform Mitigation Mechanism |
| --- | --- |
| **Malicious or Compromised Driver initiates DMA Overwrite** | The hardware IOMMU blocks any DMA write access outside the explicit guest physical pages mapped into the driver’s allocated `VMO` buffers. |
| **Driver attempts MMIO snooping of other peripherals** | The microkernel maps strictly the validated BAR address ranges into the `devhost`'s virtual address space (`VMAR`). Probing outside returns `ZX_ERR_ACCESS_DENIED` or causes an immediate page fault. |
| **Rogue Driver Attempts Network Exfiltration** | `devhost` execution domains are denied `bexos.net.SocketProvider` handles. Drivers cannot establish network sessions; they can only pass raw packet buffers to `networkd`. |
| **Device Firmware Supply Chain Poisoning** | Driver binaries and peripheral firmware images are signed OCI artifacts verified via TUF before execution. Downgrades are blocked using hardware monotonic counters in Trusty RPMB. |
| **Interrupt Flooding (Denial of Service)** | The kernel tracks interrupt dispatch rates per `zx.Handle:INTERRUPT`. If a broken or malicious driver fails to de-assert hardware IRQ lines, the kernel throttles or disables the vector and alerts `driver_manager`. |

---

## 11. Implementation Roadmap

### Phase 1: Core Framework & PCIe Bus Enumerator

* Build `driver_manager` component with ACPI and PCIe root-complex bus enumerators.
* Implement `devhost` runner capable of executing dynamically mapped shared-object driver targets.
* Implement kernel capability wrappers for `zx.Handle:INTERRUPT` and MMIO `VMO` ranges.

### Phase 2: High-Performance Network Data Plane

* Finalize `NetworkDeviceDataPlane` FIDL shared-memory ring interface.
* Implement standard Intel (`e1000e`, `igb`) and VirtIO-Net drivers targeting `libdriver`.
* Wire zero-copy packet exchange directly between `devhost` instances and `networkd`.

### Phase 3: Dynamic Packaging via `pkgd` & Heart Transplant

* Connect `driver_manager` to `pkgd` for on-demand driver resolution and installation.
* Implement automated `devhost` crash detection, channel stashing, and rapid state restoration without reboot.
