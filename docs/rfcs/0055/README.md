# RFC 0055: On-demand driver provisioning

- Created: 2026-09-04T09:32:51-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

Appd resolves unmatched hardware through deterministic identifiers, trusted registry queries, and TUF-verified driver packages. Scoped resource handoff and deployment policy constrain activation.

## Design overview

On-demand hardware provisioning resolves driver and firmware packages through BexOS's capability and cryptographic model. Like package-based provisioning in Linux distributions, it installs support when needed. The pipeline defines a well-known URL serialization format and extensions to FIDL and TUF manifests.

## Hardware Match Identifier Serialization

When a bus driver (e.g., `xhcid`, `pcie_root`, or platform device tree scanner) registers a node via `RegisterDeviceNode` and `appd` cannot find a local driver package in `/system/drivers` or `/data/drivers`, `appd` serializes `DeviceNodeInfo` into a deterministic **Hardware Match Query Path**.

### Deterministic Canonical Format

#### PCI Bus

```text
pci/v{vendor_id:04x}:d{device_id:04x}/sv{subvendor:04x}:sd{subdevice:04x}/bc{class:02x}:sc{subclass:02x}:pi{prog_if:02x}

```


*Example (Intel Wi-Fi 6 AX200):*
`pci/v8086:d2723/sv8086:s0084/bc02:sc80:pi00`

#### USB Bus

```text
usb/v{vendor_id:04x}:p{product_id:04x}/rev{device_release:04x}/bc{bDeviceClass:02x}:sc{bDeviceSubClass:02x}:pr{bDeviceProtocol:02x}

```


*Example (Realtek USB Ethernet):*
`usb/v0bda:p8153/rev0300/bc00:sc00:pr00`

#### Platform / Device Tree (`PLATFORM_DT`)

```text
dt/{compatible_string_sanitized}/rev{revision:04x}

```


*Example (Rockchip I2C Controller):*
`dt/rockchip,rk3588-i2c/rev0000`

## The Trusted Registry URL Resolution Protocol

Users configure one or more trusted driver endpoints via `appd` policy:

```toml
# /system/config/driver_sources.toml
[[trusted_registries]]
name = "bexos-official"
url = "https://drivers.bexos.org/v1"
tuf_root = "/system/etc/tuf/bexos-drivers-root.json"
priority = 100

[[trusted_registries]]
name = "enterprise-corp"
url = "https://registry.corp.internal/bexos/drivers/v1"
tuf_root = "/data/corp/tuf-root.json"
priority = 50

```

### Query Flow

When resolution triggers for `pci/v8086:d2723/sv8086:s0084/bc02:sc80:pi00`:

#### Query URL Construction

```http
GET https://drivers.bexos.org/v1/lookup/pci/v8086:d2723/sv8086:s0084/bc02:sc80:pi00/manifest.json

```


#### Fallback / Relaxation Cascade

If the full path returns a 404, `appd` walks up the specificity tree:
* Try exact match: `pci/v8086:d2723/sv8086:s0084/bc02:sc80:pi00`
* Relax Subsystem: `pci/v8086:d2723/generic`
* Generic Class Driver: `pci/class/bc02:sc80:pi00` (e.g., standard AHCI or xHCI)

## TUF Targets Metadata for Driver Packages

The query does not directly return executable ELF code. It returns an authenticated TUF target mapping.

In the repository's `targets.json` (or a delegated `targets/drivers.json`), custom target metadata links hardware fingerprints to OCI or `.bex` package digests:

```json
{
  "signatures": [...],
  "signed": {
    "_type": "targets",
    "spec_version": "1.0.3",
    "version": 42,
    "expires": "2026-10-01T00:00:00Z",
    "targets": {
      "drivers/net/iwlwifi-ax200-1.4.2.bex": {
        "length": 482910,
        "hashes": {
          "sha256": "8f3b49e1..."
        },
        "custom": {
          "bexos_driver": {
            "bus": "pci",
            "matches": [
              "pci/v8086:d2723/*",
              "pci/v8086:d271b/*"
            ],
            "required_resources": [
              { "kind": "MMIO", "min_count": 1 },
              { "kind": "INTERRUPT", "min_count": 1 },
              { "kind": "DMA_POOL", "min_count": 1 }
            ],
            "package_oci_ref": "registry.bexos.org/drivers/net/iwlwifi:1.4.2@sha256:8f3b49e1..."
          }
        }
      }
    }
  }
}

```

## Architectural Sequence: Discovery to Execution

```
┌──────────┐         ┌──────────┐         ┌──────────┐        ┌──────────────────┐
│ Bus      │         │ `appd`   │         │ Trusted  │        │ Isolated Driver  │
│ Driver   │         │ Registry │         │ TUF Repo │        │ Process (D1)     │
└────┬─────┘         └────┬─────┘         └────┬─────┘        └────────┬─────────┘
     │                    │                    │                       │
     │ 1. RegisterDevice(node_info, res)       │                       │
     ├───────────────────►│                    │                       │
     │                    │                    │                       │
     │                    │ 2. Local lookup fails                      │
     │                    │ 3. Serialize query URL                     │
     │                    │    GET /lookup/pci/v8086...                │
     │                    ├───────────────────►│                       │
     │                    │                    │                       │
     │                    │ 4. Verify TUF signatures                   │
     │                    │    & fetch target blob (OCI/TUF)           │
     │                    │◄───────────────────┤                       │
     │                    │                                            │
     │                    │ 5. Validate driver ELF against Trusty TEE  │
     │                    │                                            │
     │                    │ 6. Spawn driver in sandboxed D1 Job        │
     │                    ├───────────────────────────────────────────►│
     │                    │                                            │
     │                    │ 7. Hand off HardwareResource handles       │
     │                    │    (MMIO, Interrupt, DMA Pool)             │
     │                    ├───────────────────────────────────────────►│
     │                    │                                            │
     │ 8. Status: OK      │                                            │
     │◄───────────────────┤                                            │

```

## FIDL Protocol Extensions

The following protocol extensions support remote resolution and driver binding without blocking the bus enumerator:

```fidl
library bexos.hardware.manager;

using bexos.kernel;

// ... [Existing enums: Status, BusType, HardwareResourceKind, StopReason] ...
// ... [Existing structs: DeviceProperty, DeviceNodeInfo, HardwareResource] ...

enum DriverSourceKind : uint8 {
    LOCAL_STORAGE = 1;
    TRUSTED_TUF_REGISTRY = 2;
    USER_PROMPTED = 3;
};

struct DriverResolutionStatus {
    node_id uint64;
    matched_driver_id string:64;
    source DriverSourceKind;
    version string:32;
};

@discoverable
protocol DeviceRegistry {
    /// Registers a hardware node and kicks off local/remote resolution asynchronously.
    /// Returns immediately once the bus accepts registration, preventing bus enumeration stalls.
    RegisterDeviceNode(resource struct {
        info DeviceNodeInfo;
        resources vector<HardwareResource>:16;
    }) -> (struct {
        status Status;
    });

    UnregisterDeviceNode(struct {
        node_id uint64;
    }) -> (struct {
        status Status;
    });

    /// Administrative interface for configuring remote driver repos
    RegisterDriverSource(struct {
        name string:32;
        url string:256;
        tuf_root_digest array<uint8, 32>;
        allow_dynamic_fetch bool;
    }) -> (struct {
        status Status;
    });
};

/// Interface implemented by the spawning driver process
protocol DriverInstance {
    /// Hand off hardware capabilities after sandbox startup
    BindHardware(resource struct {
        node_id uint64;
        resources vector<HardwareResource>:16;
    }) -> (struct {
        status Status;
    });
};

protocol DriverLifecycle {
    PrepareStop(struct {
        reason StopReason;
    }) -> (struct {
        status Status;
    });
};

```

## Security Guarantees

* **Least Privilege Hardware Hand-Off:** The downloaded driver never touches general hardware. It receives *only* the specific `HardwareResource` handles (`MMIO`, `INTERRUPT`, `DMA_POOL`) passed by `appd`. Even if a compromised driver is fetched, it is confined to the MMIO physical ranges of that single peripheral.
* **Firmware Extraction Safety:** If the driver requires firmware blobs (e.g., Wi-Fi MAC microcode), `appd` passes those blobs as read-only, non-executable VMOs verified against the same TUF repository manifest.
* **Enterprise Air-Gapping:** Setting `allow_dynamic_fetch = false` on production machines forces `appd` to resolve strictly from the read-only `/system/drivers` partition, preventing unsolicited outbound network calls on sensitive deployments.
