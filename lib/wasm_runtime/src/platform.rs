//! Wasmtime's BexOS platform ABI and owned guest memory/stack mappings.
#[cfg(any(bexos_guest, test))]
mod region;

#[cfg(bexos_guest)]
mod bexos {
    use super::region::{MappingBackend, Region};
    use bexos_userspace::{Memory, syscall};
    use std::{cell::Cell, ops::Range, sync::Arc};
    use wasmtime::{
        Config, LinearMemory, MemoryCreator, MemoryType, Result, StackCreator, StackMemory, bail,
    };
    #[thread_local]
    static TLS: Cell<*mut u8> = Cell::new(core::ptr::null_mut());
    #[unsafe(no_mangle)]
    extern "C" fn wasmtime_tls_get(slot: usize) -> *mut u8 {
        assert_eq!(slot, 0);
        TLS.get()
    }
    #[unsafe(no_mangle)]
    extern "C" fn wasmtime_tls_set(slot: usize, value: *mut u8) {
        assert_eq!(slot, 0);
        TLS.set(value);
    }
    pub fn configure(config: &mut Config, max_memory: usize) {
        config.with_host_memory(Arc::new(Creator { max_memory }));
        config.with_host_stack(Arc::new(Creator { max_memory }));
    }
    struct Creator {
        max_memory: usize,
    }
    struct KernelMappings;
    impl MappingBackend for KernelMappings {
        fn reserve(&mut self, bytes: usize) -> Result<(u64, usize)> {
            Memory::create_sub_vmar(syscall::heap_vmar(), 0, bytes as u64, 3 | 8 | 0x20)
                .map(|(arena, base)| (arena, base as usize))
                .map_err(|e| wasmtime::format_err!("create wasm VMAR: {e:?}"))
        }
        fn create(&mut self, bytes: usize) -> Result<u64> {
            Memory::create(bytes as u64, 0)
                .map_err(|e| wasmtime::format_err!("create wasm VMO: {e:?}"))
        }
        fn map(&mut self, arena: u64, vmo: u64, offset: usize, bytes: usize) -> Result<()> {
            Memory::map_vmo(arena, vmo, 0, offset as u64, bytes as u64, 3)
                .map(|_| ())
                .map_err(|e| wasmtime::format_err!("map wasm memory: {e:?}"))
        }
        fn destroy(&mut self, arena: u64) {
            let _ = Memory::destroy_vmar(arena);
        }
        fn close(&mut self, vmo: u64) {
            let _ = Memory::close(vmo);
        }
    }
    // Each region exclusively owns its mapping. Wasmtime serializes mutation
    // through the Store; no host alias is retained or exposed to a guest.
    unsafe impl LinearMemory for Region<KernelMappings> {
        fn byte_size(&self) -> usize {
            self.size
        }
        fn byte_capacity(&self) -> usize {
            self.capacity
        }
        fn as_ptr(&self) -> *mut u8 {
            self.base as *mut u8
        }
        fn grow_to(&mut self, size: usize) -> Result<()> {
            self.grow(size)
        }
    }
    unsafe impl MemoryCreator for Creator {
        fn new_memory(
            &self,
            ty: MemoryType,
            minimum: usize,
            maximum: Option<usize>,
            reserved: Option<usize>,
            guard: usize,
        ) -> Result<Box<dyn LinearMemory>, String> {
            let max = maximum.unwrap_or(self.max_memory).min(self.max_memory);
            if ty.is_shared()
                || ty.is_64()
                || minimum > max
                || guard != 0
                || reserved.is_some_and(|v| v > max)
            {
                return Err("unsupported memory request".into());
            }
            Region::new(KernelMappings, minimum, max)
                .map(|r| Box::new(r) as _)
                .map_err(|e| e.to_string())
        }
    }
    // The first and last pages of the VMAR are deliberately unmapped.
    unsafe impl StackMemory for Region<KernelMappings> {
        fn top(&self) -> *mut u8 {
            (self.base + self.capacity) as *mut u8
        }
        fn range(&self) -> Range<usize> {
            self.base..self.base + self.capacity
        }
        fn guard_range(&self) -> Range<*mut u8> {
            (self.base - 4096) as *mut u8..self.base as *mut u8
        }
    }
    unsafe impl StackCreator for Creator {
        fn new_stack(&self, size: usize, _zeroed: bool) -> Result<Box<dyn StackMemory>> {
            if size > 4 << 20 {
                bail!("host stack limit");
            }
            Ok(Box::new(Region::new(KernelMappings, size, size)?))
        }
    }
}
/// Installs the shared BexOS linear-memory and Pulley-stack backend.
/// Embedded runtimes use the same mapping invariants as the application runner.
pub fn configure(config: &mut wasmtime::Config, max_memory: usize) {
    #[cfg(bexos_guest)]
    bexos::configure(config, max_memory);
    #[cfg(not(bexos_guest))]
    let _ = (config, max_memory);
}
