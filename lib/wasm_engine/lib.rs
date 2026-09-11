//! Identical Pulley compiler settings for Bazel artifacts and device engines.
use wasmtime::{Config, Result};

pub fn config(limits: &bexos_wasm_abi::Limits) -> Result<Config> {
    limits
        .validate()
        .map_err(|e| wasmtime::format_err!("invalid limits: {e:?}"))?;
    let mut config = Config::new();
    config.target("pulley64")?;
    // Updated components still compile on device; bound their startup cost.
    config.cranelift_opt_level(wasmtime::OptLevel::None);
    config.consume_fuel(true);
    config.wasm_memory64(false);
    config.signals_based_traps(false);
    config.memory_reservation(0);
    // The independent growth allowance otherwise reserves another 2 GiB.
    config.memory_reservation_for_growth(0);
    config.memory_guard_size(0);
    config.memory_init_cow(false);
    config.max_wasm_stack(limits.max_stack_bytes as usize);
    config.async_stack_size(limits.max_stack_bytes as usize * 2);
    Ok(config)
}
