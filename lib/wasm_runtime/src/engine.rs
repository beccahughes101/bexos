use wasmtime::{Engine, Result};

/// No native code, native signal traps, ambient WASI, or guest threads.
pub fn engine() -> Result<Engine> {
    configured_engine(&bexos_wasm_abi::Limits::default())
}

pub fn configured_engine(limits: &bexos_wasm_abi::Limits) -> Result<Engine> {
    let mut config = bexos_wasm_engine::config(limits)?;
    crate::platform::configure(&mut config, limits.max_memory_pages as usize * 65536);
    Engine::new(&config)
}

#[cfg(test)]
mod tests {
    #[test]
    fn executes_pulley() {
        let engine = super::engine().unwrap();
        let bytes = wat::parse_str("(module (func (export \"answer\") (result i32) i32.const 42))")
            .unwrap();
        let module = wasmtime::Module::new(&engine, bytes).unwrap();
        let mut store = wasmtime::Store::new(&engine, ());
        store.set_fuel(1000).unwrap();
        let instance = wasmtime::Instance::new(&mut store, &module, &[]).unwrap();
        assert_eq!(
            instance
                .get_typed_func::<(), i32>(&mut store, "answer")
                .unwrap()
                .call(&mut store, ())
                .unwrap(),
            42
        );
    }
}
