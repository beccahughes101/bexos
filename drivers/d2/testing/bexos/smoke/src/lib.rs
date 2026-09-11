#![no_std]

pub fn wasm_platform_smoke() -> usize {
    core::mem::size_of::<usize>()
}
