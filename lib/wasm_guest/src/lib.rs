//! Generated component bindings and checked adapters for BexOS WASM applications.
wit_bindgen::generate!({
    path: ["external/+_repo_rules+wasmtime_wasi_wit/src/p2/wit", "lib/wasm_runtime/wit"],
    world: "bexos:wasm/service",
    pub_export_macro: true,
    generate_all,
});
pub mod tty;

pub mod command;
