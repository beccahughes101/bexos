use crate::{
    context::Context,
    resources::{Entry, Handle},
};
use std::sync::Arc;
use wasmtime::{
    Result, bail,
    component::{Resource, ResourceTable},
};
#[derive(Default)]
pub struct State {
    pub budget: Option<Arc<crate::budget::Budget>>,
    pub table: ResourceTable,
    pub ids: std::collections::BTreeSet<u32>,
    pub restored: std::collections::BTreeMap<u32, super::checkpoint::Saved>,
    pub count: usize,
}
#[derive(Clone)]
pub struct Descriptor {
    pub entry: Entry,
    pub flags: super::wasi::filesystem::types::DescriptorFlags,
}
#[derive(Clone)]
pub struct DirectoryStream {
    pub entries: std::collections::VecDeque<super::wasi::filesystem::types::DirectoryEntry>,
}
#[derive(Clone)]
pub enum Input {
    Empty,
    File(Entry, u64),
    Socket(Arc<dyn Handle>),
}
pub use super::output_state::Output;
#[derive(Clone)]
pub enum Pollable {
    Output(Output),
    Ready,
    Timer(u64),
    Socket(Arc<dyn Handle>, bool),
    Network(Arc<dyn Handle>, bool, bool),
}
#[derive(Clone)]
pub struct IoError(pub String);
pub struct TerminalInput;
pub struct TerminalOutput;
impl Context {
    pub(crate) fn reserve_resources(&self, count: usize) -> Result<ResourceReservation> {
        let budget = self
            .wasi
            .budget
            .as_ref()
            .expect("instance handle budget")
            .clone();
        if !budget.charge_handles(count) {
            bail!("aggregate WASI handle limit");
        }
        Ok(ResourceReservation {
            budget,
            remaining: count,
        })
    }
    pub(crate) fn push_reserved<T: Send + 'static>(
        &mut self,
        value: T,
        reservation: &mut ResourceReservation,
    ) -> Result<Resource<T>> {
        if reservation.remaining == 0
            || !Arc::ptr_eq(&reservation.budget, self.wasi.budget.as_ref().unwrap())
        {
            bail!("invalid WASI resource reservation");
        }
        let r = self.wasi.table.push(value)?;
        reservation.remaining -= 1;
        self.wasi.ids.insert(r.rep());
        self.wasi.count += 1;
        Ok(r)
    }
    pub(crate) fn push<T: Send + 'static>(&mut self, value: T) -> Result<Resource<T>> {
        let mut reservation = self.reserve_resources(1)?;
        self.push_reserved(value, &mut reservation)
    }
    pub(crate) fn push_pair<A: Send + 'static, B: Send + 'static>(
        &mut self,
        a: A,
        b: B,
        reservation: &mut ResourceReservation,
    ) -> Result<(Resource<A>, Resource<B>)> {
        let first = self.push_reserved(a, reservation)?;
        match self.push_reserved(b, reservation) {
            Ok(second) => Ok((first, second)),
            Err(error) => {
                self.delete(first)?;
                Err(error)
            }
        }
    }
    pub(crate) fn delete<T: Send + 'static>(&mut self, r: Resource<T>) -> Result<T> {
        let id = r.rep();
        let v = self.wasi.table.delete(r)?;
        self.wasi.ids.remove(&id);
        self.wasi.count -= 1;
        self.wasi
            .budget
            .as_ref()
            .expect("instance handle budget")
            .release_handles(1);
        Ok(v)
    }
}

impl Drop for State {
    fn drop(&mut self) {
        if let Some(b) = &self.budget {
            b.release_handles(self.count);
        }
    }
}

/// Charges the complete operation before it can consume a kernel resource.
pub(crate) struct ResourceReservation {
    budget: Arc<crate::budget::Budget>,
    remaining: usize,
}
impl Drop for ResourceReservation {
    fn drop(&mut self) {
        self.budget.release_handles(self.remaining);
    }
}
