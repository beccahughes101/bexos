# RFC 0052: USB controller and device services

- Created: 2026-09-04T08:39:09-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

USB uses separate D1 controller, bus-management, and class-driver responsibilities. Scoped channels, realtime ring processing, shared transfer buffers, and device policy support the long-term host and device-mode design.

## Design overview

USB should **not** be implemented as a single monolithic service or kernel driver. In a microkernel architecture like BexOS, USB is a multi-tier pipeline with conflicting performance constraints: host controller ring manipulation requires low-latency, deterministic processing, while device class drivers (HID, Mass Storage, Audio, CDC-Ethernet) require modularity, fault isolation, and permission gating.

Current implementation status is documented in [USB implementation guide](../../usb.md).
This design remains the full long-term target. Host mode, boot HID, SCSI
bulk-only storage, descriptor policy, scoped interface channels, and migration
records have initial D1 implementation. AArch64 QEMU USB traffic is not yet
validated: the smoke target builds and launches with emulated xHCI/HID/hub
devices, but the guest did not reach USB readiness markers on 2026-09-09.
Device mode, MSI-X delivery,
isochronous scheduling, general HID parsing, UAS, audio/video class drivers,
CDC-Ethernet, stronger physical-device authorization, x86_64 guest validation,
and physical-hardware validation remain future work.

USB should be decomposed into a **two-tier D1 service model** with dedicated **real-time worker threads for transfer ring servicing**.

## The Two-Tier D1 Decomposition

Split USB into a low-level **Host Controller Driver (`xhcid`)** and a high-level **USB Device Bus Manager (`usbd`)**:

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ CLASS DRIVERS (Independent D1 User Processes)                               │
│                                                                             │
│   [ hid_driver ]       [ ustorage_driver ]     [ uaudio_driver ]            │
│   (Keyboard/Touch)     (Flash Drives/VFS)      (Microphone/DAC)             │
└──────────┬──────────────────────┬──────────────────────┬────────────────────┘
           │                      │                      │ Scoped Device Handles
           ▼                      ▼                      ▼ (Isoch / Bulk Pipes)
┌─────────────────────────────────────────────────────────────────────────────┐
│ `usbd` (USB Bus & Device Manager — D1 Service)                              │
│                                                                             │
│ • Enumeration engine: Reads Descriptors (Device, Config, Interface)         │
│ • Topology tracking: Root hubs, external hubs, port power management        │
│ • Device-driver matchmaking and permission capability routing               │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ Raw Transfer Request Blocks (TRBs)
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ `xhcid` (Host Controller Driver — High-Priority D1 Service)                 │
│                                                                             │
│  [ Control / Mgmt Thread ]   ◄──►   [ Real-Time Ring Worker Thread ]        │
│  • MSI-X & Interrupt Ports          • Schedules Isochronous/Bulk rings      │
│  • Root Hub Status Changes          • Direct DMA mapping to physical VMOs   │
│                                     • Deadline-scheduled for frame pacing   │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ Physical MMIO & Bus-Mastering DMA
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ D0 MICROKERNEL (Hardware Access Primitives)                                 │
│ • `zx.Handle:INTERRUPT` (MSI-X IRQ vector bound to `xhcid` event port)      │
│ • `zx.Handle:VMO_CONTIGUOUS` (Pinned physical memory for xHCI rings/scratch)│
│ • `zx.Handle:MMIO` (Memory-mapped xHCI register space)                      │
└─────────────────────────────────────────────────────────────────────────────┘

```

### Rationale for separating `xhcid` and `usbd`

* **Fault Isolation:** An errant device enumeration loop or a buggy third-party accessory driver cannot crash the host controller interface or interrupt other connected devices.
* **Driver Surface Security:** Individual class drivers (e.g., `ustorage_driver`) do not hold DMA bus-mastering capabilities or MMIO access to xHCI registers. Only `xhcid` touches hardware registers.

## Realtime threading model

Realtime threads are required only for xHCI event and transfer ring processing.

USB traffic encompasses four distinct transfer types with drastically different latency profiles:

| Transfer Type | Examples | Scheduling Class | Justification |
| --- | --- | --- | --- |
| **Isochronous** | USB DACs, UVC Webcams | **Hard Real-Time (Deadline / FIFO)** | Dropped microframes (125 µs intervals in USB 2.0/3.0) cause immediate audio popping, frame jitter, or buffer underruns. |
| **Interrupt** | Mice, Keyboards, Touch digitizers | **High-Priority Soft Real-Time** | Input latency directly impacts user perceived responsiveness (1000 Hz polling mice must be serviced every 1 ms). |
| **Bulk** | Flash drives, NVMe enclosures | **Standard Throughput (Fair Share)** | Latency insensitive; prioritizes high DMA burst bandwidth. |
| **Control** | Device setup, descriptor reads | **Normal Interactive** | One-off request-response cycles. |

### Implementing the Real-Time Ring Worker

In `xhcid`, configure a dedicated worker thread with a **SCHED_DEADLINE** or microkernel real-time priority slice:

1. **Interrupt Servicing:** The microkernel binds the xHCI MSI-X interrupt vector to a BexOS event port. When the host controller raises an interrupt, the real-time thread wakes with minimal context-switch jitter.
2. **Ring Pointer Advancement:** The thread reads the Event Ring, processes completed Transfer Request Blocks (TRBs), updates the Event Ring Dequeue Pointer (ERDP), and fires completion notifications back to the respective class drivers.
3. **Isochronous Ring Pre-Pacing:** The thread ensures isochronous transfer rings always have descriptors queued 2–4 subframes in advance to prevent controller ring starvation.

## Zero-Copy Data Path: Avoiding Microkernel IPC Overhead

Transferring high-throughput data (such as a 4K webcam feed or multi-gigabyte/sec USB3 NVMe transfer) by copying memory across IPC channels would degrade performance.

Use **pinned VMO sharing**:

1. The consumer process (e.g., `camera_service` or `vfsd`) allocates a Virtual Memory Object (`VMO`).
2. The consumer grants `xhcid` access to this VMO via an IPC channel.
3. `xhcid` resolves the underlying physical pages using microkernel memory pinning primitives (`vmo_pin`), generates the scatter-gather TRB chain directly pointing to those physical memory frames, and submits them to the xHCI transfer ring.
4. The xHCI controller directly executes bus-mastering DMA into/out of the user buffer without intermediate copying.

## Security & The Malicious Peripheral Threat (BadUSB)

Exposing raw USB devices to standard userspace or automatically executing code on hotplug is a historical security failure of desktop operating systems.

* **Capability-Gated Driver Binding:** When a device is plugged in, `usbd` reads the device and interface descriptors. It queries `appd` to see if an authorized class driver exists.
* **USB Firewall Policy:** Untrusted USB devices (e.g., a flash drive that suddenly enumerates a secondary HID keyboard interface to inject keystrokes) are held in an unconfigured state. `usbd` rejects unexpected composite interfaces unless authenticated via device policy or user prompt.
* **IOMMU Isolation:** `xhcid` must execute with its PCI device bound to an isolated IOMMU domain. If a compromised USB peripheral triggers malicious DMA attacks, the hardware IOMMU blocks access to physical memory outside the explicitly pinned transfer VMOs.

* The USB implementation should support both host and device mode.
