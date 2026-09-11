# RFC 0003: Native and compatibility gaming

- Created: 2026-08-26T17:31:23-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

BexOS uses WASM and WebGPU for native games and an isolated, hardware-virtualized MicroVM for legacy Proton and Windows games.

## Design overview

The gaming design pairs **WASM + WebGPU as the Tier 1 native target** with an **isolated, hardware-virtualized MicroVM for legacy Proton/Windows games**.

Related approaches include ChromeOS **Borealis (Steam on ChromeOS)** and macOS **Apple Hypervisor + Game Porting Toolkit**. The compatibility environment preserves the core OS architecture while supporting legacy games.

## The Dual-Track Gaming Architecture

```
+=========================================================================+
|                        BexOS Gaming Topology                            |
+=========================================================================+

  [ TRACK 1: Tier 1 Native ]                  [ TRACK 2: Quarantined MicroVM ]
  Native WASM + WebGPU Games                  Steam / Proton / Windows Games
  (Bevy, Godot 4, Unreal 5 WASM)              (DirectX 11/12, Win32 binaries)
              |                                             |
              v (Zero-overhead WASI/WMO)                    v (virtio-gpu / Venus)
  +-----------------------+                   +---------------------------+
  |  WASM Gaming Runner   |                   |  MicroVM Guest (Arch/Alpine|
  |  (Direct WebGPU API)  |                   |  + Proton + Steam Client) |
  +-----------------------+                   +---------------------------+
              |                                             |
              | (Direct Draw Commands)                      | (Shared-Memory Surfaces)
              +---------------------+-----------------------+
                                    |
                                    v
  +-----------------------------------------------------------------------+
  |               BexOS Compositor & D1 Vulkan Driver                     |
  +-----------------------------------------------------------------------+

```

## Track 1: Native WASM + WebGPU (The Future)

Using WebGPU and WASM as the platform's primary graphics and compute API provides the following benefits:

* **Engine Parity:** Modern engines (Bevy, Godot 4.x, PlayCanvas, and Unreal experimental) export directly to `wasm32-wasip1` / `wasm64` with WebGPU render pipelines.
* **Write Once, Run on Any Silicon:** A `.bexapp` game runs identically on an x86 AMD/NVIDIA desktop, an ARM64 Snapdragon X Elite laptop, or an Apple Silicon SOC without cross-compiling binaries.
* **Direct Hardware Speed:** Unlike in-browser gaming, a native BexOS WASM runtime avoids JavaScript event-loop hops. WebGPU API calls compile directly down to the D1 Vulkan/Metal hardware driver with near-zero driver overhead.

## Track 2: The MicroVM Proton Runner (Legacy Compatibility)

For existing Windows games, running an isolated Linux MicroVM via hardware virtualization (KVM/EL2 Hypervisor) solves the legacy problem without compromising the microkernel:

* **Fast Boot & Sleep:** A stripped-down Linux microkernel boots in `< 200 ms` via a lightweight VMM (like `crosvm` or a custom Rust VMM). It can be spun up only when Steam launches and suspended to RAM when idle.
* **VirtIO-GPU Venus Protocol:** Vulkan commands from DXVK / VKD3D inside the VM are serialized and passed through a shared-memory **VirtIO-GPU (Venus)** channel directly to the BexOS host Vulkan driver. Games achieve **~95–98% native graphics throughput**.
* **Zero Host Pollution:** The guest VM has no access to the BexOS capability table, `/data` home folders, or the host filesystem. It only sees a single virtual display surface and an isolated virtual game disk image.
* **Unbreakable Heart Transplants:** Because the hypervisor runs as a standard userspace service holding DMA VMOs, the BexOS microkernel underneath can undergo a live **Heart Transplant** update while the guest VM continues rendering frames uninterrupted.

## Summary Comparison

| Feature | Track 1: Native WASM + WebGPU | Track 2: MicroVM + Proton |
| --- | --- | --- |
| **Target Workloads** | Next-gen games, indie titles, emulators | Steam library, AAA Windows titles |
| **Portability** | **100% ISA-independent** | Tied to host CPU architecture (or uses Box64/FEX) |
| **Sandbox Security** | Fine-grained capability security | Hardware hypervisor boundary |
| **Overhead** | Sub-millisecond launch, zero VM overhead | ~200 MB base RAM footprint for VM |
