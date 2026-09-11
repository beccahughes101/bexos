use crate::{
    context::Context,
    control::{Control, Controlled},
    instance::compile_component,
    wasi::Command,
};
use std::sync::Arc;
use wasmtime::{
    Engine, Result, Store,
    component::{HasSelf, Linker},
};
pub struct CommandInstance {
    pub store: Store<Context>,
    pub command: Command,
    pub control: Arc<Control>,
    pub bytes: Arc<[u8]>,
}
impl CommandInstance {
    pub async fn instantiate(engine: &Engine, bytes: Arc<[u8]>, context: Context) -> Result<Self> {
        let component = compile_component(engine, &bytes, context.options.limits.max_module_bytes)?;
        let mut linker = Linker::new(engine);
        Command::add_to_linker::<_, HasSelf<_>>(
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
        let command = Controlled::new(
            control.clone(),
            Command::instantiate_async(&mut store, &component, &linker),
        )
        .await?;
        Ok(Self {
            store,
            command,
            control,
            bytes,
        })
    }
    pub async fn run(&mut self) -> Result<u8> {
        match Controlled::new(
            self.control.clone(),
            self.command.wasi_cli_run().call_run(&mut self.store),
        )
        .await
        {
            Ok(Ok(())) => Ok(0),
            Ok(Err(())) => Ok(1),
            Err(e) => match e.downcast_ref::<crate::wasi::cli::Exit>() {
                Some(exit) => Ok(exit.0),
                None => Err(e),
            },
        }
    }
}
