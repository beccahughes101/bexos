use crate::{
    budget::{Budget, Limiter},
    host::Host,
    resources::{Origin, Resources},
};
use bexos_wasm_abi::WasmRunnerOptions;
use std::sync::Arc;
pub struct Context {
    pub restoring: bool,
    pub checkpoint_deferred: bool,
    pub insecure_seed: Option<(u64, u64)>,
    pub budget: Arc<Budget>,
    pub child_permit: Option<crate::budget::ChildPermit>,
    pub children: std::collections::BTreeMap<u32, Box<crate::child::Child>>,
    pub next_child: u32,
    pub wasi: crate::wasi::state::State,
    pub options: WasmRunnerOptions,
    pub resources: Resources,
    pub host: Arc<dyn Host>,
    pub limiter: Limiter,
    pub origin: Origin,
    pub messages: std::collections::VecDeque<Vec<u8>>,
    pub component_dependencies: Vec<crate::migration::ComponentPayload>,
}
impl Context {
    /// Monotonic IDs encode retired children without retaining their stores.
    pub fn child_status(&self, id: u32) -> Option<u8> {
        self.children
            .get(&id)
            .map(|child| child.control().status())
            .or_else(|| (id != 0 && id < self.next_child).then_some(2))
    }
    pub fn new(
        options: WasmRunnerOptions,
        host: Arc<dyn Host>,
        origin: Origin,
        budget: Arc<Budget>,
    ) -> Self {
        let handles = Budget::with_handle_parent(&options.limits, Some(budget.clone()));
        Self {
            restoring: false,
            insecure_seed: None,
            checkpoint_deferred: false,
            budget: budget.clone(),
            child_permit: None,
            children: Default::default(),
            next_child: 1,
            wasi: crate::wasi::state::State {
                budget: Some(handles.clone()),
                table: Default::default(),
                count: 0,
                ids: Default::default(),
                restored: Default::default(),
            },
            resources: Resources::new(options.limits.max_handles as usize, origin)
                .with_budget(handles),
            limiter: Limiter::new(budget, options.limits.clone()),
            options,
            host,
            origin,
            messages: Default::default(),
            component_dependencies: Vec::new(),
        }
    }
}
