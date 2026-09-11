# RFC 0036: WASM runtime and driver architecture

- Created: 2026-08-30T14:51:24-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

A shared WASM runtime supports standalone WASI applications and restricted embedded filters. Explicit checkpoints support replacement, while the broader peripheral-driver model remains a future extension.

## Implementation status

The shared runtime uses Wasmtime 48.0.1 and Cranelift to compile ordinary WASM
modules and components into Pulley bytecode on-device. Serialized engine
artifacts are not accepted. The native embedding uses a BexOS memory, stack,
and TLS adapter. See [current runtime documentation](../../wasm-runtime.md)
for implemented interfaces, limits, and test status. Appd supports BootFS and disk
launches under the consumer identity, a separately verified native runner, WASI
0.2, and versioned core/component service interfaces. Guest-service and trusted
runner replacement use explicit checkpoints and kernel resource adoption;
live children retain their state across successful cutover.

Drivers, native JIT execution, and QuickJS compilation/packaging remain future
work. The driver sections below preserve that longer-term design; appd currently
rejects WASM driver launches. VMARs organize mappings, while WASM validation and
checked host interfaces enforce the in-process guest boundary.

The WASM runtime architecture in BexOS splits into a **shared core engine crate/library (`bexos-wasm-runtime`)** that is configured differently depending on whether it is running as an embedded, in-process filter or as a full out-of-process application runner.

## The Core Shared Runtime Crate (`bexos-wasm-runtime`)

A shared, modular Rust crate wraps Wasmtime and provides separate engine, capability, WASI, sandbox, and migration modules. Alternative engines and native JIT execution would need their own platform adapters and validation:

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ `bexos-wasm-runtime` (Shared Core Engine Crate)                             │
├─────────────────────────────────────────────────────────────────────────────┤
│ Core Modules:                                                               │
│ • `engine`: Instance lifecycle, memory sandbox, fuel/epoch watchdog         │
│ • `wit_binder`: WebAssembly Component Model dynamic dispatch               │
│ • `wasi_shim`: Pluggable WASI capability virtualization layer               │
│ • `bexos_abi`: FIDL/kernel handle Marshalling hostcalls                     │
└──────────────────────────────────────┬──────────────────────────────────────┘
                   ┌───────────────────┴───────────────────┐
                   ▼                                       ▼
┌──────────────────────────────────────┐ ┌────────────────────────────────────┐
│ In-Process Embedded Engine           │ │ Out-of-Process App Runner          │
│ (e.g., inside `netstack`, `jobd`)    │ │ (`bexos-wasm-runner` / `appd`)     │
├──────────────────────────────────────┤ ├────────────────────────────────────┤
│ • Zero WASI (Pure computation)       │ │ • Standard WASI (Virtually bridged)│
│ • Fuel strictly metered (5k ops)     │ │ • Full BexOS FIDL + Kernel Channels│
│ • Bounded lifecycle              │ │ • Sandboxed VFS namespaces         │
└──────────────────────────────────────┘ └────────────────────────────────────┘

```

## WASI support by execution model

Standalone applications use WASI; embedded filters do not.

### A. Embedded In-Process Filters (e.g., `netstack` network hooks, `jobd` parsers)

* Embedded filters must not use WASI.
* Embedded filters should export a pure function (e.g., via WIT) and import only minimal hostcalls (e.g., monotonic time, logging).
* Enabling WASI filesystem or socket imports inside `netstack` introduces ambient authority risks and unnecessary overhead for stateless packet inspection.

### B. Full Standalone Applications (`bexos-wasm-runner`)

* Standalone applications use WASI 0.2 / WASI Component Model Preview 2, virtualized over BexOS capabilities.
* **Why use WASI?** Standard toolchains (Rust `wasm32-wasip2`, C/C++, Go, AssemblyScript, Zig) target WASI out-of-the-box. Supporting WASI lets developers compile off-the-shelf libraries and codebases without rewriting standard I/O logic.
* **How WASI is bridged:** The runner maps standard WASI calls to BexOS microkernel primitives:
* `wasi:cli/environment` $\longrightarrow$ Read from the process startup environment table injected by `appd`.
* `wasi:filesystem/types` $\longrightarrow$ Bridged directly to `vfsd` capability channels mapped to `/pkg`, `/data`, and `/deps`.
* `wasi:sockets/tcp` $\longrightarrow$ Bridged to `netstack` FIDL channels (`bexos.net.Netstack`).
* `wasi:clocks/monotonic-clock` $\longrightarrow$ Directly reads the mapped fast Time VMO.

## Application Process Layout (`bexos-wasm-runner`)

When `appd` launches a WASM-based package (`manifest.proto` specifies `runner = "wasm"`), `appd` spawns the generic `bexos-wasm-runner` process:

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ WASM APP PROCESS (`bexos-wasm-runner`)                                      │
├─────────────────────────────────────────────────────────────────────────────┤
│ 1. Startup:                                                                 │
│    • Receives startup namespace handles: `/pkg`, `/data`, `/deps`           │
│    • Receives startup FIDL bootstrap channel from `appd`                    │
│                                                                             │
│ 2. Hostcall Layer (BexOS ABI + WASI Bridge):                                │
│    • `__bexos_channel_call()` ──► Native `sys_channel_call` syscall         │
│    • `wasi:filesystem/read`   ──► Reads from `/data` via `vfsd` channel     │
│    • `wasi:sockets/connect`   ──► Calls `netstack` via FIDL                 │
│                                                                             │
│ 3. Linear Memory Sandbox:                                                   │
│    • App WASM bytecode (Loaded from `/pkg/app.wasm` via ArchiveFS)          │
│    • Isolated heap & stack with guard pages                                 │
└─────────────────────────────────────────────────────────────────────────────┘

```

## Custom BexOS Hostcalls vs. WASI

WASI provides standard POSIX-like convenience, while native BexOS hostcalls provide first-class capability access:

```rust
// In-process host function table exposed to WASM guest
pub fn register_bexos_hostcalls(linker: &mut wasmtime::Linker<ProcessContext>) {
    // 1. Direct handle-based IPC
    linker.func_wrap("bexos:kernel/ipc", "channel_write", |ctx: Caller<'_, ProcessContext>, handle: u32, buf_ptr: u32, buf_len: u32| {
        // Transfers data directly from WASM linear memory to the kernel channel
    })?;

    // 2. Direct fast-path clock read
    linker.func_wrap("bexos:kernel/time", "get_monotonic_ns", |ctx: Caller<'_, ProcessContext>| -> u64 {
        ctx.data().time_vmo.read_monotonic_ns()
    })?;
}

```

## Design summary

* Build a single **`bexos-wasm-runtime`** crate with feature flags (`embedded` vs. `runner`).
* **Embedded filters:** Zero WASI, strict fuel limits, WIT-defined packet/event transform functions.
* **App Runner:** WASI 0.2 Component Model backed by virtualized BexOS VFS, `netstack`, and `keychain` capabilities, giving apps full language ecosystem compatibility while preserving microkernel sandboxing.

**Future driver design:** WASM-based device drivers (such as HID mice, keyboards, touchscreens, or USB gamepads) run in userspace under a driver host process (`bexos-driver-host-wasm`). They interface with hardware transport pipes on the bottom edge and expose standardized FIDL protocols to the OS on the top edge.

## Future: The WASM Driver Architecture

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ SYSTEM INPUT SERVICE (`inputd` / UI Compositor)                             │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ Standard FIDL: `bexos.input.report.InputDevice`
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ D1 WASM DRIVER HOST (`bexos-driver-host-wasm`)                              │
│                                                                             │
│  ┌───────────────────────────────────────────────────────────────────────┐  │
│  │ Guest WASM Linear Memory (e.g., `hid-generic-mouse.wasm`)              │  │
│  │                                                                       │  │
│  │ • Contains compiled HID report descriptor parser                      │  │
│  │ • Unpacks raw byte streams into typed report structures               │  │
│  │ • Implements FIDL serialization via generated guest bindings          │  │
│  └───────────────────▲───────────────────────────────┬───────────────────┘  │
│                      │                               │                      │
│        Hostcalls     │ Direct Memory View            │ Hostcalls            │
│  ┌───────────────────┴───────────────────────────────▼───────────────────┐  │
│  │ Host Runtime Shims (bexos-wasm-runtime)                               │  │
│  │ • `bexos:driver/transport`: Reads raw packets from USB/I2C pipe      │  │
│  │ • `bexos:kernel/channel`: Writes serialized FIDL messages to inputd   │  │
│  └───────────────────────────────────────────────────────────────────────┘  │
└──────────────────────────────────────▲──────────────────────────────────────┘
                                       │ Hardware Transport: `bexos.hardware.usb.Endpoint`
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ USB / I2C CONTROLLER DRIVER (e.g., `d1_xhci`)                               │
└─────────────────────────────────────────────────────────────────────────────┘

```

## How WASM Drivers Expose FIDL

WASM modules interact with FIDL without special microkernel magic by using **Guest-Side FIDL Codegen + Handle Hostcalls**:

1. **FIDL Compilation Target:** The `fidlc` compiler generates guest bindings (e.g., Rust `no_std` or C) targeted for WASM.
2. **Buffer Serialization:** When a button is clicked, the WASM HID parser formats the event into a standard FIDL message directly inside its linear memory buffer.
3. **Channel Hostcalls:** The driver invokes a hostcall function to push that buffer down the outgoing kernel channel.

```rust
// Guest-side Rust code inside `hid_mouse.wasm`
#[no_mangle]
pub extern "C" fn on_transport_data_ready(raw_len: usize) {
    let raw_bytes = unsafe { TRANSPORT_BUFFER.get_slice(raw_len) };

    // 1. Parse raw HID packet into structured report
    let event = parse_hid_mouse_report(raw_bytes);

    // 2. Encode to FIDL message bytes
    let mut fidl_buf = [0u8; 128];
    let encoded_len = bexos_input_report::serialize_mouse_event(&event, &mut fidl_buf);

    // 3. Send over server channel endpoint via hostcall
    unsafe {
        bexos_abi::channel_write(
            CLIENT_CHANNEL_HANDLE,
            fidl_buf.as_ptr(),
            encoded_len as u32,
            core::ptr::null(), // No handles transferred
            0
        );
    }
}

```

## Standard HID FIDL Protocol (`bexos.input.report`)

The FIDL protocol exposed by WASM HID drivers to `inputd` mirrors standard event/report descriptors:

```fidl
library bexos.input.report;

using bexos.kernel;

type MouseInputReport = struct {
    buttons_mask uint32;
    movement_x int32;
    movement_y int32;
    scroll_delta int32;
};

type KeyboardInputReport = struct {
    pressed_keys vector<uint32>:16; // HID key usage codes
};

type TouchInputReport = struct {
    contact_id uint32;
    x int32;
    y int32;
    pressure uint32;
};

type InputReport = strict union {
    1: mouse MouseInputReport;
    2: keyboard KeyboardInputReport;
    3: touch TouchInputReport;
};

@discoverable
protocol InputDevice {
    /// Retrieve static capabilities and descriptor
    GetDescriptor() -> (struct {
        status bexos.kernel.Status,
        vendor_id uint32,
        product_id uint32,
        device_type string:64
    });

    /// Stream input reports to subscriber (e.g. inputd)
    ReadReport() -> (struct {
        status bexos.kernel.Status,
        report InputReport,
        event_time uint64
    });

    /// Alternative high-speed path: bind a kernel FIFO for zero-copy streaming
    BindReportFifo(resource struct {
        fifo handle:FIFO
    }) -> (struct {
        status bexos.kernel.Status
    });
};

```

## Hostcall Surface for WASM Drivers (`bexos:driver`)

WASM drivers import a constrained set of hostcalls provided by the driver host:

```wit
package bexos:driver;

interface transport {
    /// Read raw hardware packets (from USB interrupt in-endpoint or I2C ring)
    read-raw-transfer: func(max-bytes: u32) -> list<u8>;

    /// Send control requests to hardware (e.g. Set_Report / LED state)
    write-control-transfer: func(request-type: u8, request: u8, value: u16, data: list<u8>) -> u32;
}

interface ipc {
    /// Write serialized FIDL bytes to a connected client
    channel-write: func(channel-handle: u32, bytes: list<u8>, handles: list<u32>) -> u32;

    /// Receive messages on the driver's listening endpoint
    channel-read: func(channel-handle: u32) -> tuple<list<u8>, list<u32>>;
}

```

## Why WASM for Peripheral Drivers?

* **Parser isolation goal:** Out-of-bounds guest memory accesses should trap the WASM instance. This depends on correct validation, engine execution, and checked host interfaces; it does not eliminate driver-host or kernel bugs.
* **Driver replacement goal:** Preserve the physical transport through explicit lifecycle hooks and resource adoption. Startup latency and connectivity preservation require driver-specific tests; no sub-millisecond claim is established.
* **Zero Host Toolchain Dependency:** A hardware vendor can distribute a single, architecture-neutral `driver.wasm` binary that runs without recompilation on x86_64, AArch64, or RISC-V BexOS systems.
