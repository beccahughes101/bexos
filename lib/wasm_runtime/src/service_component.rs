use crate::{
    bindings::Service,
    context::Context,
    control::{Control, Controlled},
    instance::compile_component,
};
use std::sync::Arc;
use wasmtime::{
    Engine, Result, Store, bail,
    component::{HasSelf, Linker},
};
pub struct ServiceInstance {
    pub store: Store<Context>,
    pub service: Service,
    pub control: Arc<Control>,
    pub bytes: Arc<[u8]>,
}
impl ServiceInstance {
    pub async fn instantiate(engine: &Engine, bytes: Arc<[u8]>, context: Context) -> Result<Self> {
        let component = compile_component(engine, &bytes, context.options.limits.max_module_bytes)?;
        Self::instantiate_compiled(&component, bytes, context).await
    }
    pub(crate) async fn instantiate_compiled(
        component: &wasmtime::component::Component,
        bytes: Arc<[u8]>,
        context: Context,
    ) -> Result<Self> {
        let engine = component.engine();
        let mut linker = Linker::new(engine);
        crate::wasi::Command::add_to_linker::<_, HasSelf<_>>(
            &mut linker,
            &crate::wasi::LinkOptions::default(),
            |ctx| ctx,
        )?;
        crate::bindings::bexos::wasm::kernel::add_to_linker::<_, HasSelf<_>>(&mut linker, |ctx| {
            ctx
        })?;
        crate::bindings::bexos::wasm::ui::add_to_linker::<_, HasSelf<_>>(&mut linker, |ctx| ctx)?;
        crate::component_sandbox::link(&mut linker)?;
        crate::bindings::bexos::wasm::checkpoint_resources::add_to_linker::<_, HasSelf<_>>(
            &mut linker,
            |ctx| ctx,
        )?;
        let fuel = context.options.limits.fuel;
        let slice = context.options.limits.fuel_slice;
        let stack = context.options.limits.max_stack_bytes as usize;
        let mut store = Store::try_new_with_pulley_stack(engine, context, stack)?;
        store.limiter(|ctx| &mut ctx.limiter);
        store.set_fuel(fuel)?;
        store.fuel_async_yield_interval(Some(slice))?;
        let control = Arc::new(Control::default());
        let service = Controlled::new(
            control.clone(),
            Service::instantiate_async(&mut store, &component, &linker),
        )
        .await?;
        Ok(Self {
            store,
            service,
            control,
            bytes,
        })
    }
    pub async fn dispatch(&mut self, id: u32) -> Result<()> {
        Controlled::new(
            self.control.clone(),
            self.service
                .bexos_wasm_lifecycle()
                .call_dispatch(&mut self.store, id),
        )
        .await
    }
    pub async fn checkpoint(&mut self) -> Result<Vec<u8>> {
        self.store.data_mut().checkpoint_deferred = false;
        let bytes = self
            .service
            .bexos_wasm_lifecycle()
            .call_checkpoint(&mut self.store)
            .await?;
        if self.store.data().checkpoint_deferred {
            bail!("guest deferred checkpoint");
        }
        if bytes.len() > crate::lifecycle::MAX_CHECKPOINT {
            bail!("checkpoint limit");
        }
        Ok(bytes)
    }
    pub async fn restore_checkpoint(&mut self, bytes: &[u8]) -> Result<()> {
        if bytes.len() > crate::lifecycle::MAX_CHECKPOINT {
            bail!("checkpoint limit");
        }
        if self
            .service
            .bexos_wasm_lifecycle()
            .call_restore(&mut self.store, bytes)
            .await?
            .is_err()
        {
            bail!("guest rejected checkpoint");
        }
        Ok(())
    }
    pub async fn activate(&mut self) -> Result<()> {
        self.store.data_mut().restoring = false;
        self.service
            .bexos_wasm_lifecycle()
            .call_activate(&mut self.store)
            .await
    }
    pub async fn abort_migration(&mut self) -> Result<()> {
        self.service
            .bexos_wasm_lifecycle()
            .call_abort(&mut self.store)
            .await
    }
}
