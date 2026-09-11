//! Shared output state keeps stream pollables and migration checkpoints aligned.
use crate::resources::{Entry, Handle};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};
pub const CAPACITY: usize = 32768;
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
#[derive(Clone)]
pub enum Target {
    Closed,
    Log,
    File(Entry, u64),
    Socket(Arc<dyn Handle>),
}
#[derive(Clone)]
pub struct OutputState {
    pub target: Target,
    pub pending: Vec<u8>,
    pub permit: usize,
    pub flushing: bool,
    pub failure: Option<String>,
}
#[derive(Clone)]
pub struct Output {
    id: u64,
    state: Arc<Mutex<OutputState>>,
}
impl Output {
    fn new(target: Target) -> Self {
        Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            state: Arc::new(Mutex::new(OutputState {
                target,
                pending: Vec::new(),
                permit: 0,
                flushing: false,
                failure: None,
            })),
        }
    }
    pub fn closed() -> Self {
        Self::new(Target::Closed)
    }
    pub fn log() -> Self {
        Self::new(Target::Log)
    }
    pub fn file(entry: Entry, offset: u64) -> Self {
        Self::new(Target::File(entry, offset))
    }
    pub fn socket(handle: Arc<dyn Handle>) -> Self {
        Self::new(Target::Socket(handle))
    }
    pub fn id(&self) -> u64 {
        self.id
    }
    pub fn snapshot(&self) -> OutputState {
        self.state.lock().unwrap().clone()
    }
    pub fn restore(id: u64, state: OutputState) -> wasmtime::Result<Self> {
        if id == 0
            || id == u64::MAX
            || state.pending.len() > CAPACITY
            || state.permit > CAPACITY - state.pending.len()
            || (state.flushing && state.permit != 0)
            || state.failure.as_ref().is_some_and(|s| s.len() > 4096)
        {
            wasmtime::bail!("invalid output checkpoint");
        }
        NEXT_ID.fetch_max(id + 1, Ordering::Relaxed);
        Ok(Self {
            id,
            state: Arc::new(Mutex::new(state)),
        })
    }
    pub(crate) fn lock(&self) -> std::sync::MutexGuard<'_, OutputState> {
        self.state.lock().unwrap()
    }
}
