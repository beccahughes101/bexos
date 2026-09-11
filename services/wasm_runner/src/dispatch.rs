//! Native services receive a bounded execution allowance for each dispatch.
//! A long-lived process must not consume one lifetime budget merely by polling.
use bexos_wasm_runtime::service_guest::ServiceGuest;

pub fn run(
    instance: &mut ServiceGuest,
    resource: u32,
    pending: impl FnMut(),
) -> wasmtime::Result<()> {
    let fuel = instance.store().data().options.limits.fuel;
    instance.store_mut().set_fuel(fuel)?;
    crate::executor::block_on_with(instance.dispatch(resource), pending)
}

#[cfg(test)]
mod tests {
    #[test]
    fn repeated_service_dispatches_live_but_a_runaway_dispatch_traps() {
        use bexos_wasm_runtime::{
            budget::Budget, context::Context, instance::CoreInstance, resources::Origin,
        };
        use std::sync::Arc;
        let mut options = bexos_wasm_abi::WasmRunnerOptions::default();
        options.limits.fuel = 128;
        options.limits.fuel_slice = 32;
        let engine = bexos_wasm_runtime::engine::configured_engine(&options.limits).unwrap();
        let context = Context::new(
            options,
            Arc::new(crate::host::NativeHost::new()),
            Origin::Signed,
            Budget::new(64 << 20),
        );
        let bytes = wat::parse_str(
            r#"(module
            (func (export "bexos-service-dispatch") (param i32)
                local.get 0
                if (loop $spin br $spin) end))"#,
        )
        .unwrap();
        let core =
            crate::executor::block_on(CoreInstance::instantiate(&engine, bytes.into(), context))
                .unwrap();
        let mut guest = bexos_wasm_runtime::service_guest::ServiceGuest::Core(core);
        for _ in 0..1000 {
            super::run(&mut guest, 0, || {}).unwrap();
        }
        assert!(super::run(&mut guest, 1, || {}).is_err());
        assert_eq!(guest.store().get_fuel().unwrap(), 0);
    }
}
