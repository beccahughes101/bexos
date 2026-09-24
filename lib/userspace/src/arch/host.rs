//! Host test stubs deliberately contain no guest syscall instructions.
pub fn fidl(
    _protocol: u64,
    _ordinal: u64,
    _request: &[u8],
    _handles: &[u64],
    _response: &mut [u8],
    _out_handles: &mut [u64],
) -> Result<(usize, usize), i32> {
    Err(-1)
}
pub fn yield_now() {}
pub fn commit_transplant() {}
pub fn log(_message: &str) {}
pub fn exit() -> ! {
    panic!("guest exit called from a host test")
}
pub fn ticks() -> u64 {
    0
}
pub fn frequency() -> u64 {
    1
}
pub fn heap_vmar() -> u64 {
    0
}
pub fn pci_ecam_base() -> u64 {
    0
}
pub fn pci_segment() -> u16 {
    0
}
pub fn pci_bus_range() -> (u8, u8) {
    (0, 0)
}
pub fn pci_mmio_window() -> (u64, u64) {
    (0, 0)
}
pub fn cmos(_register: u8, _value: Option<u8>) -> Result<u8, i32> {
    Err(-1)
}
pub fn thread_pointer() -> u64 {
    0
}

pub fn console_frame(_bytes: &[u8]) -> Result<(), i32> {
    Err(-1)
}
