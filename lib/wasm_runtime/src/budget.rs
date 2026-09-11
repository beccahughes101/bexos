use bexos_wasm_abi::Limits;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use wasmtime::{ResourceLimiter, Result};
/// Shared aggregate memory budget across every instance in an application.
pub struct Budget {
    handle_parent: Option<Arc<Budget>>,
    used: AtomicU64,
    maximum: u64,
    children: AtomicU64,
    handles: AtomicU64,
    max_handles: u64,
    table_elements: AtomicU64,
    max_table_elements: u64,
}
impl Budget {
    pub fn new(maximum: u64) -> Arc<Self> {
        let mut limits = Limits::default();
        limits.max_memory_pages = maximum / 65536;
        Self::for_limits(&limits)
    }
    pub fn for_limits(limits: &Limits) -> Arc<Self> {
        Self::with_handle_parent(limits, None)
    }
    /// An instance shares one handle quota across core and canonical WASI tables.
    pub fn with_handle_parent(limits: &Limits, handle_parent: Option<Arc<Budget>>) -> Arc<Self> {
        Arc::new(Self {
            handle_parent,
            used: AtomicU64::new(0),
            children: AtomicU64::new(0),
            maximum: limits.max_memory_pages * 65536,
            handles: AtomicU64::new(0),
            max_handles: limits.max_handles,
            table_elements: AtomicU64::new(0),
            max_table_elements: limits.max_table_elements,
        })
    }
    pub(crate) fn charge_handles(&self, n: usize) -> bool {
        if self
            .handle_parent
            .as_ref()
            .is_some_and(|parent| !parent.charge_handles(n))
        {
            return false;
        }
        let charged = self
            .handles
            .try_update(Ordering::AcqRel, Ordering::Acquire, |v| {
                v.checked_add(n as u64).filter(|v| *v <= self.max_handles)
            })
            .is_ok();
        if !charged {
            if let Some(parent) = &self.handle_parent {
                parent.release_handles(n);
            }
        }
        charged
    }
    pub(crate) fn release_handles(&self, n: usize) {
        self.handles.fetch_sub(n as u64, Ordering::AcqRel);
        if let Some(parent) = &self.handle_parent {
            parent.release_handles(n);
        }
    }
    fn charge(&self, n: u64) -> bool {
        self.used
            .try_update(Ordering::AcqRel, Ordering::Acquire, |old| {
                old.checked_add(n).filter(|new| *new <= self.maximum)
            })
            .is_ok()
    }
    fn release(&self, n: u64) {
        self.used.fetch_sub(n, Ordering::AcqRel);
    }
}
pub struct Limiter {
    budget: Arc<Budget>,
    charged: u64,
    pending: u64,
    table_charged: u64,
    table_pending: u64,
    limits: Limits,
}
impl Limiter {
    pub fn new(budget: Arc<Budget>, limits: Limits) -> Self {
        Self {
            budget,
            charged: 0,
            pending: 0,
            table_charged: 0,
            table_pending: 0,
            limits,
        }
    }
}
impl ResourceLimiter for Limiter {
    fn memory_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> Result<bool> {
        self.pending = 0;
        if desired > self.limits.max_memory_pages as usize * 65536
            || maximum.is_some_and(|v| desired > v)
        {
            return Ok(false);
        }
        let n = desired.saturating_sub(current) as u64;
        if !self.budget.charge(n) {
            return Ok(false);
        }
        self.charged += n;
        self.pending = n;
        Ok(true)
    }
    fn memory_grow_failed(&mut self, _error: wasmtime::Error) -> Result<()> {
        self.budget.release(self.pending);
        self.charged -= self.pending;
        self.pending = 0;
        Ok(())
    }
    fn table_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> Result<bool> {
        self.table_pending = 0;
        if desired as u64 > self.limits.max_table_elements || maximum.is_some_and(|m| desired > m) {
            return Ok(false);
        }
        let n = desired.saturating_sub(current) as u64;
        if self
            .budget
            .table_elements
            .try_update(Ordering::AcqRel, Ordering::Acquire, |v| {
                v.checked_add(n)
                    .filter(|v| *v <= self.budget.max_table_elements)
            })
            .is_err()
        {
            return Ok(false);
        }
        self.table_pending = n;
        self.table_charged += n;
        Ok(true)
    }
    fn table_grow_failed(&mut self, _error: wasmtime::Error) -> Result<()> {
        self.budget
            .table_elements
            .fetch_sub(self.table_pending, Ordering::AcqRel);
        self.table_charged -= self.table_pending;
        self.table_pending = 0;
        Ok(())
    }
    fn instances(&self) -> usize {
        64
    }
    fn memories(&self) -> usize {
        16
    }
    fn tables(&self) -> usize {
        16
    }
}
impl Drop for Limiter {
    fn drop(&mut self) {
        self.budget.release(self.charged);
        self.budget
            .table_elements
            .fetch_sub(self.table_charged, Ordering::AcqRel);
    }
}

pub struct ChildPermit(Arc<Budget>);
impl Drop for ChildPermit {
    fn drop(&mut self) {
        self.0.children.fetch_sub(1, Ordering::AcqRel);
    }
}
impl Budget {
    pub fn child(self: &Arc<Self>, maximum: u64) -> wasmtime::Result<ChildPermit> {
        if self
            .children
            .try_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                n.checked_add(1).filter(|v| *v <= maximum)
            })
            .is_err()
        {
            wasmtime::bail!("aggregate child limit");
        }
        Ok(ChildPermit(self.clone()))
    }
}
