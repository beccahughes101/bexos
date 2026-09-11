use crate::{
    context::Context,
    control::{Control, Controlled},
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use wasmtime::{Engine, Instance, Linker, Module, Result, Store, bail};
static COMPILING: AtomicBool = AtomicBool::new(false);
struct Compilation;
impl Drop for Compilation {
    fn drop(&mut self) {
        COMPILING.store(false, Ordering::Release);
    }
}
pub fn validate_bytes(bytes: &[u8], maximum: u64) -> Result<()> {
    if bytes.len() < 8 || bytes.len() as u64 > maximum || !bytes.starts_with(b"\0asm") {
        bail!("expected bounded raw WebAssembly bytes");
    }
    Ok(())
}
pub fn compile_module(engine: &Engine, bytes: &[u8], maximum: u64) -> Result<Module> {
    validate_bytes(bytes, maximum)?;
    if COMPILING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        bail!("compiler busy");
    }
    let _guard = Compilation;
    Module::new(engine, bytes)
}
pub fn compile_component(
    engine: &Engine,
    bytes: &[u8],
    maximum: u64,
) -> Result<wasmtime::component::Component> {
    validate_bytes(bytes, maximum)?;
    if let Some(component) = bexos_wasm_embedded::component(engine, bytes) {
        #[cfg(bexos_guest)]
        bexos_userspace::log("wasm_runtime: loading trusted precompiled component\n");
        return component;
    }
    if COMPILING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        bail!("compiler busy");
    }
    let _guard = Compilation;
    let component = wasmtime::component::Component::new(engine, bytes)?;
    #[cfg(bexos_guest)]
    bexos_userspace::log("wasm_runtime: component compilation complete\n");
    Ok(component)
}
pub struct CoreInstance {
    pub store: Store<Context>,
    pub instance: Instance,
    pub control: Arc<Control>,
    pub bytes: Arc<[u8]>,
}
impl CoreInstance {
    pub async fn instantiate(engine: &Engine, bytes: Arc<[u8]>, context: Context) -> Result<Self> {
        let module = compile_module(engine, &bytes, context.options.limits.max_module_bytes)?;
        Self::instantiate_compiled(&module, bytes, context).await
    }
    pub(crate) async fn instantiate_compiled(
        module: &Module,
        bytes: Arc<[u8]>,
        context: Context,
    ) -> Result<Self> {
        let engine = module.engine();
        let mut linker = Linker::new(engine);
        crate::abi::link(&mut linker)?;
        let fuel = context.options.limits.fuel;
        let slice = context.options.limits.fuel_slice;
        let stack = context.options.limits.max_stack_bytes as usize;
        let mut store = Store::try_new_with_pulley_stack(engine, context, stack)?;
        store.limiter(|ctx| &mut ctx.limiter);
        store.set_fuel(fuel)?;
        store.fuel_async_yield_interval(Some(slice))?;
        let control = Arc::new(Control::default());
        let instance = Controlled::new(
            control.clone(),
            linker.instantiate_async(&mut store, &module),
        )
        .await?;
        Ok(Self {
            store,
            instance,
            control,
            bytes,
        })
    }
    pub async fn invoke(&mut self, name: &str, input: i32) -> Result<i32> {
        let function = self
            .instance
            .get_typed_func::<i32, i32>(&mut self.store, name)?;
        Controlled::new(
            self.control.clone(),
            function.call_async(&mut self.store, input),
        )
        .await
    }
    pub async fn run(&mut self) -> Result<()> {
        let function = self
            .instance
            .get_typed_func::<(), ()>(&mut self.store, "_start")?;
        Controlled::new(
            self.control.clone(),
            function.call_async(&mut self.store, ()),
        )
        .await
    }
}
