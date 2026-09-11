# RFC 0048: Process heaps and virtual memory

- Created: 2026-09-01T13:21:13-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

Userspace allocators grow process heaps through anonymous VMOs inside VMAR reservations. Kernel page commitment and capability-scoped mappings provide the backing and isolation primitives.

## Design overview

Process heap management in BexOS operates without legacy POSIX `brk`/`sbrk` mechanisms. Instead, heaps are structured around **VMAR (Virtual Memory Address Region) sub-allocations backed by anonymous VMOs (Virtual Memory Objects)** and managed in userspace by modern, allocator-friendly memory engines.

Current implementation status: the active bare-metal runtime creates a root VMAR
and a fixed-position read/write heap VMAR reservation for each process. Appd
constructs native ELF processes with child VMARs for image, library, TLS, stack,
and guard regions, while the userspace and BexOS libc allocators grow by
creating anonymous VMOs and mapping 2MB chunks into the heap VMAR. Anonymous VMO
backing is lazy: uncommitted reads use the kernel zero page, lower-EL writes
commit one physical frame, and DMA pinning materializes a contiguous copy when
required. Full ASLR, allocator replacement with mimalloc/jemalloc, MTE
integration, and 1TB heap reservations remain future design work.

## Process Memory Address Space (VMAR Layout)

When `app_service` spawns a process, the microkernel initializes a root `VMAR` capability with randomized base offsets (ASLR). The userspace runtime carves this region into isolated functional sub-regions:

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ USERSPACE VIRTUAL ADDRESS SPACE (Root VMAR: 48-bit Canonical EL0)           │
├─────────────────────────────────────────────────────────────────────────────┤
│ [ 0x0000_0000_0000 ] ──► Unmapped Null Guard Region (4GB - catches traps)  │
│ [ 0x0001_0000_0000 ] ──► Executable & RO Data (.text / .rodata / .fidl)     │
│ [ 0x0002_0000_0000 ] ──► Read-Write Data (.data / .bss)                     │
│ [ 0x0003_0000_0000 ] ──► Heap VMAR Region (Sparse Address Reservation)      │
│                          ├── Committed Chunk 0 (Backing VMO mapped R/W)     │
│                          ├── Committed Chunk 1 (Backing VMO mapped R/W)     │
│                          ├── Guard Page (Unmapped PROT_NONE page)           │
│                          └── Reserved / Uncommitted Virtual Space (up to 1TB│
│ [ 0x0070_0000_0000 ] ──► Thread Stacks (with unmapped redzones/guard pages) │
│ [ 0x007F_FFFF_FFFF ] ──► Top of Canonical Userspace Range                  │
└─────────────────────────────────────────────────────────────────────────────┘

```

## Two-Tier Heap Model

### Kernel Layer (D0 Microkernel): VMO & VMAR Primitive

* **Sparse Reservation:** The kernel provides address space reservation without backing physical memory until explicitly requested. This is implemented for process heap VMARs and appd-created ELF sub-VMARs.
* **On-Demand Page Commit:** Ordinary anonymous VMOs start with per-page empty backing. Read mappings use the shared read-only zero page; a lower-EL write-permission fault on a declared-writable anonymous mapping allocates and zeroes exactly one frame, replaces the PTE, invalidates the affected translation, and retries the faulting instruction. Explicit contiguous, physical, device, and bootfs VMOs retain eager backing.
* **Handle Security:** The process holds a restricted `vmar_handle` with permissions limited to `READ | WRITE`. It cannot map memory as executable (`EXECUTE`), enforcing W^X security.

### Userspace Layer (EL0): Runtime Allocator

* **Native `std` Daemons & D1 Drivers:** Currently use the BexOS libc allocator over a 64GB heap VMAR reservation with 2MB page-aligned VMO chunks. Future ports can replace the frontend with **`mimalloc`** or **`jemalloc`** while keeping the same VMAR/VMO growth path.
* **`no_std` Drivers & Early Boot Binaries:** Use an embedded, ultra-fast allocator like **`talc`** or a slab/buddy allocator operating on a fixed pre-allocated initial VMO.
* **D2 WASM Extensions:** The embedded WASM runtime (`wasmtime` / `bexos-wasm-runtime`) allocates an isolated, bounds-checked linear memory VMAR (e.g., 4GB with guard pages). `memory.grow` instructions translate directly to resizing or committing additional pages in the WASM instance's private VMO.

## Heap Allocation & Growth Flow

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ 1. USERSPACE THREAD / HEAP ALLOCATOR                                        │
│    • Thread calls `Box::new()` or `Vec::push()`.                            │
│    • `mimalloc` arena exhausts local page arenas (e.g. 64KB slab filled).   │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ 2. Request new chunk (e.g. 2MB arena)
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ 3. BEXOS SYSTEM RUNTIME (`libstd` / allocator glue)                         │
│    • Calls `bexos.kernel.Vmo.Create(size: 2MB, flags: RESIZABLE)` via `svc #1│
│    • Calls `bexos.kernel.Vmar.Map(heap_vmar, vmo_handle, PROT_READ_WRITE)`  │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ 3. Returns virtual base address ptr
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ 4. D0 MICROKERNEL (EL1)                                                     │
│    • Allocates physical frames from Buddy Frame Allocator                   │
│    • Populates 4-level page table entries (ARM64 TTBR0_EL1)                 │
│    • Zeroes physical pages (prevents information leaks from prior processes)│
└─────────────────────────────────────────────────────────────────────────────┘

```

## Rust Allocator Integration (`libstd` / Runtime Layer)

The system allocator in Rust hooks directly into the `fidl()` syscall interface:

```rust
use core::alloc::{GlobalAlloc, Layout};
use core::ptr::NonNull;

pub struct BexOsAllocator;

const ARENA_CHUNK_SIZE: usize = 2 * 1024 * 1024; // 2MB Chunk Size

unsafe impl GlobalAlloc for BexOsAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if layout.size() > ARENA_CHUNK_SIZE {
            // Direct large mapping: Create dedicated VMO and map into VMAR
            self.map_large_vmo(layout.size(), layout.align())
        } else {
            // Forward to in-process mimalloc/slab instance
            mimalloc_sys::mi_malloc_aligned(layout.size(), layout.align()) as *mut u8
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if layout.size() > ARENA_CHUNK_SIZE {
            self.unmap_large_vmo(ptr, layout.size());
        } else {
            mimalloc_sys::mi_free(ptr as *mut _);
        }
    }
}

impl BexOsAllocator {
    unsafe fn map_large_vmo(&self, size: usize, align: usize) -> *mut u8 {
        let aligned_size = (size + 0xFFF) & !0xFFF; // Align to 4KB page

        // 1. FIDL Kernel Call: bexos.kernel.Vmo.Create
        let (vmo_handle, _) = bexos_sys::vmo_create(aligned_size as u64, 0).expect("Out of VMO memory");

        // 2. FIDL Kernel Call: bexos.kernel.Vmar.Map
        let virt_addr = bexos_sys::vmar_map_heap(vmo_handle, aligned_size as u64, bexos_sys::PROT_READ | bexos_sys::PROT_WRITE)
            .expect("VMAR Map Failed");

        virt_addr as *mut u8
    }

    unsafe fn unmap_large_vmo(&self, ptr: *mut u8, size: usize) {
        let aligned_size = (size + 0xFFF) & !0xFFF;
        let _ = bexos_sys::vmar_unmap_heap(ptr as u64, aligned_size as u64);
    }
}

#[global_allocator]
static GLOBAL: BexOsAllocator = BexOsAllocator;

```

## Security, Isolation & Diagnostic Controls

* **Mandatory Zeroing of Allocated Pages:** To prevent uninitialized memory leakage between security domains (e.g., a cryptographic daemon crashing and an untrusted app reading leftover RAM keys), the D0 microkernel clears all physical frames before mapping them into a userspace VMAR.
* **Heap Isolation & Thread Canaries:** Sub-arenas maintain guard pages marked with zero access permissions (`PROT_NONE`). Any spatial out-of-bounds heap write triggers an instant hardware MMU translation fault, suspended by the kernel and routed to `crashd`.
* **Hardware Memory Tagging (ARM64 MTE):** On compatible ARM64 cores, the allocator leverages top-byte tagging (TBT) and pointer color masks (4-bit tags in bits `[59:56]`) to catch use-after-free and off-by-one buffer overflows at hardware execution speed.
* **OOM & Process Quotas:** The kernel tracks total committed physical frames per process handle. If a process exceeds its manifest memory budget, heap mapping calls return `ERR_NO_MEMORY`, allowing the app to release caches or initiate graceful degradation rather than triggering random kernel-wide out-of-memory panics.
