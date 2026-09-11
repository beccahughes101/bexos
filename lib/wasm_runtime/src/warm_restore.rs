//! Allocate candidate stores during live preparation; cutover only installs the
//! final logical checkpoint. Never instantiate a newly discovered live child
//! while the source is quiesced.
use crate::{
    child::Child, context::Context, migration::Snapshot, prepared::PreparedService,
    resources::Resources, service_guest::ServiceGuest, wasi::state::State,
};
use std::{future::Future, pin::Pin, sync::Arc};
use wasmtime::{Engine, Result, bail};
impl Snapshot {
    pub fn prepare_candidate(
        &self,
        engine: Engine,
        mut context: Context,
        replacement: Option<Arc<[u8]>>,
        prepared: Arc<Vec<PreparedService>>,
    ) -> Pin<Box<dyn Future<Output = Result<ServiceGuest>> + '_>> {
        Box::pin(async move {
            if self.options != context.options || self.origin != context.origin {
                bail!("incompatible candidate options");
            }
            context.restoring = true;
            context.insecure_seed = self.insecure_seed;
            context.component_dependencies = self.component_dependencies.clone();
            context
                .resources
                .restore(self.next_resource, self.resources.clone())?;
            for (id, child) in &self.children {
                if *id == 0 || *id >= self.next_child || context.children.contains_key(id) {
                    bail!("invalid prepared child id");
                }
                let mut child_context = Context::new(
                    child.options.clone(),
                    context.host.clone(),
                    child.origin,
                    context.budget.clone(),
                );
                child_context.component_dependencies = child.component_dependencies.clone();
                child_context.child_permit =
                    Some(context.budget.child(context.options.limits.max_children)?);
                let instance = child
                    .prepare_candidate(engine.clone(), child_context, None, prepared.clone())
                    .await?;
                context
                    .children
                    .insert(*id, Box::new(Child::Service(instance)));
            }
            let bytes = replacement.unwrap_or_else(|| self.bytes.clone());
            prepared
                .iter()
                .find(|p| p.matches(&bytes))
                .ok_or_else(|| wasmtime::format_err!("unprepared payload"))?
                .instantiate(context)
                .await
        })
    }
    pub fn install_checkpoint(
        self,
        instance: &mut ServiceGuest,
        root: bool,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + '_>> {
        Box::pin(async move {
            if !root && self.bytes != instance.bytes() {
                bail!("child payload changed after preparation");
            }
            let ctx = instance.store().data();
            if self.options != ctx.options
                || self.origin != ctx.origin
                || !ctx.restoring
                || ctx.wasi.count != 0
            {
                bail!("candidate changed before checkpoint installation");
            }
            if self.children.len() != ctx.children.len()
                || self
                    .children
                    .iter()
                    .any(|(id, _)| !ctx.children.contains_key(id))
            {
                bail!("live child set changed after preparation");
            }
            for (id, snapshot) in self.children {
                let child = instance
                    .store_mut()
                    .data_mut()
                    .children
                    .get_mut(&id)
                    .unwrap();
                let Child::Service(child) = &mut **child else {
                    bail!("unsupported prepared child");
                };
                snapshot.install_checkpoint(child, false).await?;
            }
            let ctx = instance.store_mut().data_mut();
            let budget = ctx.wasi.budget.as_ref().unwrap().clone();
            let empty = Resources::new(ctx.options.limits.max_handles as usize, ctx.origin)
                .with_budget(budget.clone());
            drop(std::mem::replace(&mut ctx.resources, empty));
            drop(std::mem::replace(
                &mut ctx.wasi,
                State {
                    budget: Some(budget),
                    table: Default::default(),
                    ids: Default::default(),
                    restored: Default::default(),
                    count: 0,
                },
            ));
            ctx.resources.restore(self.next_resource, self.resources)?;
            ctx.wasi
                .restore(self.wasi, ctx.options.limits.max_handles as usize)?;
            ctx.next_child = self.next_child;
            ctx.insecure_seed = self.insecure_seed;
            ctx.messages = self.messages;
            ctx.component_dependencies = self.component_dependencies;
            let fuel = instance.store().get_fuel()?.min(self.remaining_fuel);
            instance.store_mut().set_fuel(fuel)?;
            instance.restore_checkpoint(&self.checkpoint).await?;
            instance
                .store_mut()
                .data_mut()
                .wasi
                .discard_unclaimed_ambient_io();
            if !instance.store().data().wasi.restored.is_empty() {
                bail!("guest did not adopt every live WASI resource");
            }
            if self.paused {
                instance.control().pause();
            } else {
                instance.control().resume();
            }
            Ok(())
        })
    }
}
