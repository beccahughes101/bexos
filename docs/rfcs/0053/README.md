# RFC 0053: I2C and SPI services

- Created: 2026-09-04T08:39:56-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

D1 bus controllers multiplex statically described peripherals through capability-scoped channels. Arbitration, realtime SPI work, and GPIO integration keep bus-specific behavior outside the kernel.

## Design overview

Current implemented behavior is tracked in [current I2C and SPI services](../../i2c_spi.md). This design document keeps the broader hardware direction and future work separate from the code that exists today.

Like USB, **I2C and SPI must not be monolithic or reside in the microkernel**.

However, their architecture differs from USB in one crucial way: USB is a dynamically enumerable bus (devices announce who they are), whereas **I2C and SPI are non-enumerable, statically routed low-speed buses** shared across multiple platform components (PMICs, touch controllers, IMUs, secure elements, ambient light sensors, display panels).

In BexOS, I2C and SPI belong in **D1 user space**, structured as **hardware bus controllers** that multiplex access for isolated **client peripheral drivers**.

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ PERIPHERAL CLIENT DRIVERS (Isolated D1 Services)                            │
│                                                                             │
│   [ touch_driver ]        [ imu_driver ]             [ pmic_manager ]       │
│   (I2C addr: 0x38)        (SPI bus 0, CS 1)          (I2C addr: 0x4B)       │
└──────────┬───────────────────────┬──────────────────────────┬───────────────┘
           │ FIDL:                 │ FIDL:                    │ FIDL:
           │ I2cDeviceChannel      │ SpiDeviceChannel         │ I2cDeviceChannel
           ▼                       ▼                          ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ BUS CONTROLLER DRIVERS (D1 Hardware Services)                               │
│                                                                             │
│  [ i2c_controller_d1 ]                  [ spi_controller_d1 ]               │
│  • Serialized FIFO transaction queue    • Real-time worker thread           │
│  • Bus locking & multi-master support   • DMA engine integration            │
│  • Address & target capability check    • Polarity/phase/baud configuration │
└───────────────────┬─────────────────────────────────┬───────────────────────┘
                    │                                 │
                    │ Physical MMIO & IRQs            │ DMA / Interrupts
                    ▼                                 ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ D0 MICROKERNEL (Zero Bus Protocol Knowledge)                                │
│ • `zx.Handle:MMIO` (Hardware controller registers)                          │
│ • `zx.Handle:INTERRUPT` (IRQ bound to async completion port)                │
│ • `zx.Handle:VMO_CONTIGUOUS` (For high-speed SPI DMA buffers)               │
└─────────────────────────────────────────────────────────────────────────────┘

```

## The Core Split: Bus Controller vs. Device Proxy

Never grant a peripheral driver (like a touchscreen driver or sensor HAL) raw MMIO access to the SoC's I2C or SPI controller.

Instead, the **Controller Driver** exposes a capability-scoped child channel:

### I2C Capability Isolation

Multiple physical chips share the same physical clock (SCL) and data (SDA) lines.

* `i2c_controller_d1` maps the physical MMIO and handles bus interrupts.
* When `devmgr` matches the board topology (from Device Tree / ACPI), it does not give the client driver the master controller handle.
* It passes an **attenuated handle** representing a virtual device channel bound strictly to an allowed 7-bit or 10-bit target address (e.g., `0x38` for FocalTech touch):
```fidl
protocol I2cDevice {
    /// Transfers are strictly targeted to the pre-bound address.
    /// The client cannot spoof another peripheral's address.
    Transfer(vector<I2cTransaction>:MAX_OPS transactions)
        -> (vector<vector<u8>>:MAX_OPS read_data) error I2cError;
};

```


### SPI Capability Isolation

SPI chips share data lines (MOSI, MISO, SCLK), but are isolated via distinct **Chip Select (CS)** lines.

* `spi_controller_d1` binds individual `SpiDevice` channels to a specific hardware or GPIO CS line.
* The peripheral driver configures bus speed, clock polarity (CPOL), and phase (CPHA) only for its assigned CS slice.

## Threading Model: Do They Need Real-Time Threads?

| Subsystem | Speed / Latency Target | Threading Strategy |
| --- | --- | --- |
| **I2C (Standard / Fast)** | 100 kHz – 1 MHz (~10–100 µs/byte) | **Normal Interactive FIFO Worker.** I2C is slow. Spinning in real-time wastes CPU. Use an event-driven loop that dispatches transactions and yields until the hardware interrupt fires via a microkernel `INTERRUPT` handle. |
| **I3C / High-Speed I2C** | 12.5 MHz – 33 MHz | **High-Priority Soft Real-Time.** When handling in-band interrupts (IBI) from dynamic sensors, service interrupts promptly to avoid bus stalling. |
| **SPI (Displays, Audio Codecs, High-Rate IMUs)** | 10 MHz – 80 MHz+ | **Dedicated Real-Time Thread + DMA Engine.** High-speed SPI transfers large payloads (e.g., streaming frames to a small sub-display or reading 1 kHz gyroscope bursts). |

### Why SPI Needs a Real-Time Thread

At 50 MHz, an SPI transfer without DMA requires clock cycles every few nanoseconds.

* **Small transfers (<64 bytes):** Handled via FIFO registers. A high-priority worker thread avoids scheduling jitter between asserting CS, blasting the FIFO, and releasing CS.
* **Large transfers (>64 bytes):** Handled via **SoC DMA engines**. The SPI controller sets up a DMA descriptor chain pointing directly to pinned `VMO` physical addresses. A real-time thread waits on the DMA completion interrupt to release the bus mutex immediately, ensuring minimal bus idle time.

## Bus Arbitration and Deadlock Prevention

Because multiple independent user-space processes talk to devices on the exact same physical I2C or SPI wires, the controller must prevent **bus contention**:

### Atomic Transaction Bundles

Sensors often require a write followed immediately by a repeated-start read without releasing the bus (e.g., `Write [Register 0x0F]` -> `Repeated START` -> `Read [1 byte]`).
* The controller accepts atomic vector transactions (`vector<I2cTransaction>`).
* It locks the controller FIFO, executes the entire bundle with repeated-starts, and only releases arbitration when the batch finishes.

### Device-Level Bus Locks

If a client driver needs exclusive access across multiple back-to-back operations (e.g., flashing an SPI NOR flash sector or reading a multi-byte burst from an IMU), the controller provides an ephemeral `LockBus()` token with a strict hardware timeout to prevent rogue drivers from hanging the bus indefinitely.

## Integration with Hardware GPIOs

I2C and SPI peripherals almost always depend on auxiliary GPIO pins:

* **Interrupt Pin (INT / IRQ):** An accelerometer pulls a GPIO low when it detects motion; a touch digitizer pulls a line low on finger contact.
* **Reset Pin (RST):** Pulsed high/low during driver initialization.

In BexOS, auxiliary pins are managed by a **`gpiod` D1 service**:

1. `gpiod` handles SoC GPIO pin controller MMIO.
2. It grants the `touch_driver` a `zx.Handle:INTERRUPT` bound to that specific GPIO pin.
3. The `touch_driver` thread blocks on this interrupt handle. When the user touches the screen, the interrupt unblocks, and the touch driver immediately dispatches an `I2cDevice.Transfer()` call to `i2c_controller_d1` to read the coordinate registers.

This architecture keeps the BexOS microkernel free from platform pin-muxing, bus protocols, and hardware quirks while guaranteeing hardware-enforced memory and capability isolation across all platform peripherals.
