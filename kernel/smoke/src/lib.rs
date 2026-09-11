#![no_std]

pub fn kernel_platform_smoke() -> usize {
    core::mem::size_of::<usize>()
}
