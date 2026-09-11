# RFC 0043: Crash handling and recovery

- Created: 2026-09-01T10:39:39-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

Crashd collects structured artifacts from exceptions, panics, and WASM traps. Capability-scoped inspection, redaction, storage quotas, and recovery policies limit exposure of sensitive process state.

## Design overview

`crashd` is the zero-ambient-authority crash handling and telemetry daemon for BexOS. It intercepts unhandled CPU exceptions, kernel panics, and WASM runtime traps, extracts structured debug artifacts, and coordinates crash recovery without leaking sensitive user data across sandbox boundaries.

## System Architecture & Crash Flow

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ FAILING PROCESS (Native D1 Driver / WASM Runtime / D2 Daemon)               │
│ • Triggers EL0 translation fault, alignment fault, or WASM trap             │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ 1. Exception Trapped
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ D0 MICROKERNEL (EL1)                                                        │
│ • Suspends faulting thread immediately                                      │
│ • Synthesizes `ExceptionReport` packet                                      │
│ • Dispatches packet to `crashd`'s registered Exception Port handle          │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ 2. Port Wait Event (`sys_port_wait`)
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ CRASHD (Userspace Crash Telemetry Daemon)                                    │
│                                                                             │
│  [ Inspection Pipeline ]                                                    │
│  • Reads thread context: Registers (PC, LR, SP, x0-x30), ESR_EL1, FAR_EL1   │
│  • Performs DWARF / WASM stack unwinding via target process VMO mappings    │
│  • Strips PII / sensitive user memory regions (Scratchpad scrubbing)       │
│                                                                             │
│  [ Mini-Dump Encoder ]                                                      │
│  • Encodes into `.bexcrash` format (zstd-compressed, content-addressed)    │
│  • Encrypts mini-dump using ephemeral Crash Public Key                      │
└───────────────────┬─────────────────────────────────────┬───────────────────┘
                    │                                     │
                    │ 3. Persist Dump                     │ 4. Recovery Signal
                    ▼                                     ▼
┌──────────────────────────────────────┐ ┌────────────────────────────────────┐
│ `vfsd` (Encrypted Storage)           │ │ `app_service` (Lifecycle Orchestr.)│
│ Writes to `/vault/crashes/<id>.dump` │ │ Decisions:                         │
│ (Bounded rotating ring: max 50MB)    │ │ • D1 Driver: Heart-transplant      │
│                                      │ │ • UI App: Restart / Error State    │
│                                      │ │ • SysUI: Fallback to Default Shell │
└──────────────────────────────────────┘ └────────────────────────────────────┘

```

## Exception Channel Protocol (`bexos.system.crashd`)

```fidl
library bexos.system.crash;

using bexos.kernel;

type ExceptionType = strict enum : uint8 {
    PAGE_FAULT_READ       = 1;
    PAGE_FAULT_WRITE      = 2;
    PAGE_FAULT_EXECUTE    = 3;
    UNDEFINED_INSTRUCTION = 4;
    UNALIGNED_ACCESS      = 5;
    WASM_TRAP_OUT_OF_BOUNDS = 6;
    WASM_TRAP_UNREACHABLE   = 7;
    WASM_FUEL_EXHAUSTED     = 8;
    MANUAL_PANIC          = 9;
};

type ThreadRegistersArm64 = struct {
    pc uint64;
    sp uint64;
    lr uint64;
    pstate uint64;
    far uint64;  // Fault Address Register
    esr uint64;  // Exception Syndrome Register
    regs array<uint64, 31>; // x0-x30
};

type CrashReportHeader = struct {
    crash_id array<uint8, 16>; // UUIDv4
    timestamp_ticks uint64;
    process_handle_koid uint64;
    package_id string:128;
    binary_sha256 array<uint8, 32>;
    exception_type ExceptionType;
    arm64_context ThreadRegistersArm64;
};

@discoverable
protocol ExceptionHandler {
    /// Microkernel delivers exception to crashd
    OnThreadException(resource struct {
        report CrashReportHeader;
        thread_handle handle:THREAD;
        process_vmar handle:VMAR;
    }) -> (struct {
        action strict enum { RESUME = 1; TERMINATE = 2; RESTART = 3; };
    });
};

```

## Core Responsibilities of `crashd`

### Out-of-Process Stack Unwinding

* `crashd` never executes inside the failing process address space.
* It uses the thread's suspended handle to read register states and the read-only process Virtual Memory Address Region (`VMAR`) handle to inspect stack pages.
* For native code, it parses the `.eh_frame` / `.debug_frame` tables. For WASM components, it translates the linear memory stack pointer against the module's function index table.

### Redaction & PII Privacy Scrubbing

* Standard raw core dumps can leak decrypted user secrets, private keys, or passwords stored in application memory.
* `crashd` enforces an **allowlist-only mini-dump strategy**:
* Captures only CPU registers, exception syndrome metadata, loaded module build IDs, and the immediate stack frame ($4\text{ KB}$ around `$SP`).
* Explicitly excludes general dynamic heap pages unless marked as debug scratchpads via `sys_vmo_set_debug_name()`.

### Ephemeral Crash Vault & Quota Management

* Crash dumps are formatted into `.bexcrash` (Zstandard compressed headers, stack slices, and backtraces).
* Saved into `/vault/system/crashes/` with strict bounded quotas:
* Maximum 20 crash dumps preserved on disk.
* Maximum disk allocation of $50\text{ MB}$; old records rotate using FIFO.

## Fault Recovery Integration Matrix

When `crashd` finishes processing a dump, it returns an action verdict to `app_service`:

| Component Crashing | Root Cause Example | `crashd` & `app_service` Recovery Action |
| --- | --- | --- |
| **D2 WASM Driver** | Out-of-bounds pointer in USB mouse driver | Resets WASM linear instance; restores driver state from manifest defaults. No process restart required. |
| **D1 Native Driver** | VirtIO GPU driver panic | Triggers driver heart-transplant via live state snapshot; if failed, restarts process and re-binds interrupt ports. |
| **System UI / Extension** | Third-party launcher panic | Traps WASM frame, disables the problematic extension, and falls back to first-party default launcher. |
| **D2 System Daemon** | `netstack` assertion failure | Reboots daemon; clients reconnect using FIDL lazy-link reconnection backoff. |
| **User WASM App** | Unhandled Rust `panic!()` | Terminates sandbox, posts mini-dump to developer telemetry queue, and prompts user: *"App closed unexpectedly."* |

## Implementation Sketch (`crashd/src/main.rs`)

```rust
#![no_std]
extern crate alloc;

use alloc::vec::Vec;
use bexos_fidl::system::crash::{CrashReportHeader, ExceptionHandler, ExceptionType};

pub struct CrashDaemon {
    port_handle: u64,
}

impl CrashDaemon {
    pub async fn run(&mut self) {
        loop {
            // 1. Await incoming exception port packet from D0 microkernel
            let (report, thread_handle, vmar_handle) = self.wait_for_exception().await;

            // 2. Perform out-of-process unwind
            let backtrace = self.unwind_stack(&report, &vmar_handle);

            // 3. Serialize and compress mini-dump
            let dump_payload = self.encode_bexcrash(&report, &backtrace);
            self.write_to_crash_vault(&report.package_id, dump_payload).await;

            // 4. Notify app_service for lifecycle mitigation
            self.notify_orchestrator(&report).await;

            // 5. Resume or terminate thread
            let _ = unsafe { bexos_sys::sys_task_kill(thread_handle) };
        }
    }

    fn unwind_stack(&self, report: &CrashReportHeader, vmar: &u64) -> Vec<u64> {
        let mut frames = Vec::new();
        let mut fp = report.arm64_context.regs[29]; // Frame pointer x29

        // Walk frame pointer chain safely through mapped VMAR
        for _ in 0..64 {
            if fp == 0 || fp & 0x7 != 0 { break; }
            let mut lr = 0u64;
            if unsafe { bexos_sys::sys_vmar_read(*vmar, fp + 8, &mut lr as *mut u64 as *mut u8, 8) }.is_err() {
                break;
            }
            frames.push(lr);
            if unsafe { bexos_sys::sys_vmar_read(*vmar, fp, &mut fp as *mut u64 as *mut u8, 8) }.is_err() {
                break;
            }
        }
        frames
    }

    fn encode_bexcrash(&self, _header: &CrashReportHeader, _bt: &[u64]) -> Vec<u8> {
        // Encodes binary header + compressed frame index
        Vec::new()
    }

    async fn wait_for_exception(&self) -> (CrashReportHeader, u64, u64) {
        // Port wait dispatch loop
        core::future::pending().await
    }

    async fn write_to_crash_vault(&self, _pkg: &str, _data: Vec<u8>) {}
    async fn notify_orchestrator(&self, _report: &CrashReportHeader) {}
}

```
