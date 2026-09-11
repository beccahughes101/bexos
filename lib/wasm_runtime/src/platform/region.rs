//! A fixed address reservation with backing added only as WASM memory grows.
use wasmtime::{Result, bail};

const PAGE: usize = 4096;
// Bound kernel page bookkeeping at startup without creating a VMO for every
// 64 KiB memory.grow. Backing pages themselves remain lazy anonymous pages.
const CHUNK: usize = 2 << 20;

pub(super) trait MappingBackend {
    fn reserve(&mut self, bytes: usize) -> Result<(u64, usize)>;
    fn create(&mut self, bytes: usize) -> Result<u64>;
    fn map(&mut self, arena: u64, vmo: u64, offset: usize, bytes: usize) -> Result<()>;
    fn destroy(&mut self, arena: u64);
    fn close(&mut self, vmo: u64);
}

pub(super) struct Region<B: MappingBackend> {
    backend: B,
    arena: u64,
    vmos: Vec<u64>,
    pub base: usize,
    pub capacity: usize,
    pub size: usize,
    mapped: usize,
}

impl<B: MappingBackend> Region<B> {
    pub fn new(mut backend: B, size: usize, capacity: usize) -> Result<Self> {
        if size > capacity {
            bail!("linear memory limit");
        }
        let capacity = capacity
            .max(PAGE)
            .checked_next_multiple_of(PAGE)
            .ok_or_else(|| wasmtime::format_err!("memory size overflow"))?;
        let reservation = capacity
            .checked_add(2 * PAGE)
            .ok_or_else(|| wasmtime::format_err!("memory size overflow"))?;
        let (arena, base) = backend.reserve(reservation)?;
        let mut region = Self {
            backend,
            arena,
            vmos: Vec::new(),
            base: base + PAGE,
            capacity,
            size: 0,
            mapped: 0,
        };
        region.grow(size)?;
        Ok(region)
    }

    pub fn grow(&mut self, size: usize) -> Result<()> {
        if size < self.size || size > self.capacity {
            bail!("linear memory limit");
        }
        let target = size
            .checked_next_multiple_of(CHUNK)
            .ok_or_else(|| wasmtime::format_err!("memory size overflow"))?
            .min(self.capacity);
        while self.mapped < target {
            let bytes = (target - self.mapped).min(CHUNK);
            let vmo = self.backend.create(bytes)?;
            if let Err(error) = self.backend.map(self.arena, vmo, self.mapped + PAGE, bytes) {
                self.backend.close(vmo);
                return Err(error);
            }
            self.vmos.push(vmo);
            self.mapped += bytes;
        }
        // Publish the new WASM length only after every mapping succeeds. If a
        // later chunk fails, retain earlier owned mappings for a safe retry.
        self.size = size;
        Ok(())
    }
}

impl<B: MappingBackend> Drop for Region<B> {
    fn drop(&mut self) {
        self.backend.destroy(self.arena);
        for vmo in &self.vmos {
            self.backend.close(*vmo);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, rc::Rc};

    #[derive(Default)]
    struct State {
        reservation: usize,
        created: Vec<usize>,
        mappings: Vec<(usize, usize)>,
        closed: Vec<u64>,
        destroyed: bool,
        fail_map: bool,
    }
    struct Fake(Rc<RefCell<State>>);
    impl MappingBackend for Fake {
        fn reserve(&mut self, bytes: usize) -> Result<(u64, usize)> {
            self.0.borrow_mut().reservation = bytes;
            Ok((42, 0x10000))
        }
        fn create(&mut self, bytes: usize) -> Result<u64> {
            let mut state = self.0.borrow_mut();
            state.created.push(bytes);
            Ok(state.created.len() as u64)
        }
        fn map(&mut self, _: u64, _: u64, offset: usize, bytes: usize) -> Result<()> {
            let mut state = self.0.borrow_mut();
            if state.fail_map {
                bail!("injected map failure");
            }
            state.mappings.push((offset, bytes));
            Ok(())
        }
        fn destroy(&mut self, _: u64) {
            self.0.borrow_mut().destroyed = true;
        }
        fn close(&mut self, vmo: u64) {
            self.0.borrow_mut().closed.push(vmo);
        }
    }

    #[test]
    fn small_memory_reserves_its_limit_without_backing_the_unused_capacity() {
        let state = Rc::new(RefCell::new(State::default()));
        let mut region = Region::new(Fake(state.clone()), 65536, 128 << 20).unwrap();
        assert_eq!(state.borrow().reservation, (128 << 20) + 2 * PAGE);
        assert_eq!(state.borrow().created, [CHUNK]);
        let base = region.base;
        region.grow(CHUNK + 65536).unwrap();
        assert_eq!(region.base, base);
        assert_eq!(region.size, CHUNK + 65536);
        assert_eq!(
            state.borrow().mappings,
            [(PAGE, CHUNK), (PAGE + CHUNK, CHUNK)]
        );
        assert!(region.grow(65536).is_err());
        assert!(region.grow((128 << 20) + 1).is_err());
        drop(region);
        assert!(state.borrow().destroyed);
        assert_eq!(state.borrow().closed, [1, 2]);
    }

    #[test]
    fn failed_growth_preserves_the_old_length_and_can_retry() {
        let state = Rc::new(RefCell::new(State::default()));
        let mut region = Region::new(Fake(state.clone()), 0, 3 * CHUNK).unwrap();
        assert!(state.borrow().created.is_empty());
        region.grow(CHUNK).unwrap();
        state.borrow_mut().fail_map = true;
        assert!(region.grow(2 * CHUNK).is_err());
        assert_eq!(region.size, CHUNK);
        assert_eq!(state.borrow().closed, [2]);
        state.borrow_mut().fail_map = false;
        region.grow(2 * CHUNK).unwrap();
        assert_eq!(
            state.borrow().mappings,
            [(PAGE, CHUNK), (PAGE + CHUNK, CHUNK)]
        );
        drop(region);
        assert_eq!(state.borrow().closed, [2, 1, 3]);
    }

    #[test]
    fn construction_failure_releases_the_reservation_and_failed_vmo() {
        let state = Rc::new(RefCell::new(State {
            fail_map: true,
            ..State::default()
        }));
        assert!(Region::new(Fake(state.clone()), PAGE, CHUNK).is_err());
        assert!(state.borrow().destroyed);
        assert_eq!(state.borrow().closed, [1]);
    }
}
