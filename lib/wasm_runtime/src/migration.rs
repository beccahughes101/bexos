//! Logical state captured at an explicit guest safe point. Resource adoption is
//! supplied by the trusted embedding and never accepts guest native handles.
use crate::{context::Context, instance::CoreInstance, resources::Entry};
use std::{future::Future, pin::Pin, sync::Arc};
use wasmtime::{Engine, Result, bail};
#[derive(Clone)]
pub struct Snapshot {
    pub options: bexos_wasm_abi::WasmRunnerOptions,
    pub origin: crate::resources::Origin,
    pub bytes: Arc<[u8]>,
    pub checkpoint: Vec<u8>,
    pub wasi: std::collections::BTreeMap<u32, crate::wasi::checkpoint::Saved>,
    pub resources: Vec<(u32, Entry)>,
    pub next_resource: u32,
    pub children: Vec<(u32, Snapshot)>,
    pub next_child: u32,
    pub messages: std::collections::VecDeque<Vec<u8>>,
    pub remaining_fuel: u64,
    pub paused: bool,
    pub insecure_seed: Option<(u64, u64)>,
    pub component_dependencies: Vec<ComponentPayload>,
}

#[derive(Clone)]
pub struct ComponentPayload {
    pub dependency: bexos_wasm_abi::ComponentDependency,
    pub bytes: Arc<[u8]>,
}
impl CoreInstance {
    pub fn snapshot(&mut self) -> Pin<Box<dyn Future<Output = Result<Snapshot>> + '_>> {
        Box::pin(async move {
            self.service_version().await?;
            let checkpoint = self.checkpoint().await?;
            let mut children = Vec::new();
            for (id, child) in &mut self.store.data_mut().children {
                children.push((*id, child.snapshot().await?));
            }
            let wasi = self.store.data_mut().wasi.snapshot()?;
            let ctx = self.store.data();
            Ok(Snapshot {
                insecure_seed: ctx.insecure_seed,
                options: ctx.options.clone(),
                origin: ctx.origin,
                bytes: self.bytes.clone(),
                checkpoint,
                wasi,
                resources: ctx
                    .resources
                    .entries()
                    .map(|(id, e)| (id, e.clone()))
                    .collect(),
                next_resource: ctx.resources.next_id(),
                children,
                next_child: ctx.next_child,
                messages: ctx.messages.clone(),
                remaining_fuel: self.store.get_fuel()?,
                paused: self.control.status() == 1,
                component_dependencies: ctx.component_dependencies.clone(),
            })
        })
    }
}
impl crate::service_guest::ServiceGuest {
    pub fn snapshot(&mut self) -> Pin<Box<dyn Future<Output = Result<Snapshot>> + '_>> {
        Box::pin(async move {
            let checkpoint = self.checkpoint().await?;
            let mut children = Vec::new();
            for (id, child) in &mut self.store_mut().data_mut().children {
                children.push((*id, child.snapshot().await?));
            }
            let wasi = self.store_mut().data_mut().wasi.snapshot()?;
            let ctx = self.store().data();
            Ok(Snapshot {
                insecure_seed: ctx.insecure_seed,
                options: ctx.options.clone(),
                origin: ctx.origin,
                bytes: self.bytes(),
                checkpoint,
                wasi,
                resources: ctx
                    .resources
                    .entries()
                    .map(|(id, e)| (id, e.clone()))
                    .collect(),
                next_resource: ctx.resources.next_id(),
                children,
                next_child: ctx.next_child,
                messages: ctx.messages.clone(),
                remaining_fuel: self.store().get_fuel()?,
                paused: self.control().status() == 1,
                component_dependencies: ctx.component_dependencies.clone(),
            })
        })
    }
}
impl Snapshot {
    pub fn restore(
        self,
        engine: Engine,
        context: Context,
        replacement: Option<Arc<[u8]>>,
    ) -> Pin<Box<dyn Future<Output = Result<crate::service_guest::ServiceGuest>>>> {
        self.restore_prepared(engine, context, replacement, None)
    }
    pub fn restore_prepared(
        self,
        engine: Engine,
        mut context: Context,
        replacement: Option<Arc<[u8]>>,
        prepared: Option<Arc<Vec<crate::prepared::PreparedService>>>,
    ) -> Pin<Box<dyn Future<Output = Result<crate::service_guest::ServiceGuest>>>> {
        Box::pin(async move {
            if self.options != context.options || self.origin != context.origin {
                bail!("incompatible restored options");
            }
            context
                .resources
                .restore(self.next_resource, self.resources)?;
            context
                .wasi
                .restore(self.wasi, context.options.limits.max_handles as usize)?;
            context.insecure_seed = self.insecure_seed;
            context.messages = self.messages;
            context.component_dependencies = self.component_dependencies.clone();
            context.next_child = self.next_child;
            for (id, snapshot) in self.children {
                if id == 0 || id >= self.next_child || context.children.contains_key(&id) {
                    bail!("invalid restored child ID");
                }
                let permit = context.budget.child(context.options.limits.max_children)?;
                let mut child_context = Context::new(
                    snapshot.options.clone(),
                    context.host.clone(),
                    snapshot.origin,
                    context.budget.clone(),
                );
                child_context.component_dependencies = snapshot.component_dependencies.clone();
                child_context.child_permit = Some(permit);
                let child = snapshot
                    .restore_prepared(engine.clone(), child_context, None, prepared.clone())
                    .await?;
                context
                    .children
                    .insert(id, Box::new(crate::child::Child::Service(child)));
            }
            context.restoring = true;
            context.options.limits.fuel = self.remaining_fuel;
            let bytes = replacement.unwrap_or(self.bytes);
            let mut instance = if let Some(prepared) = prepared {
                prepared
                    .iter()
                    .find(|compiled| compiled.matches(&bytes))
                    .ok_or_else(|| {
                        wasmtime::format_err!("live child payload changed after preparation")
                    })?
                    .instantiate(context)
                    .await?
            } else {
                crate::service_guest::ServiceGuest::instantiate(&engine, bytes, context).await?
            };
            instance.store_mut().data_mut().options = self.options;
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
            }
            Ok(instance)
        })
    }
}
