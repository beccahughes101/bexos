//! Fault injection used only by isolated recovery acceptance targets.
#[path = "recovery_staging_faults.rs"]
pub mod staging;
use bexos_trusty_boot::{
    ql::{Error, Transport},
    selection,
};
pub struct LostCommitAck<'a, T> {
    inner: &'a mut T,
    lost: bool,
}
impl<'a, T> LostCommitAck<'a, T> {
    pub fn new(inner: &'a mut T) -> Self {
        Self { inner, lost: false }
    }
}
impl<T: Transport> Transport for LostCommitAck<'_, T> {
    fn exchange(&mut self, operation: u32, length: usize, bytes: &mut [u8]) -> Result<i64, Error> {
        let commit = operation == selection::OPERATION
            && length == 256
            && bytes[8..12] == 4u32.to_le_bytes();
        let result = self.inner.exchange(operation, length, bytes)?;
        if commit && !self.lost {
            self.lost = true;
            crate::log("monitor-runtime: committed trial acknowledgement deliberately withheld\n");
            interrupt(2);
            return Err(Error::Timeout);
        }
        Ok(result)
    }
    fn now_ns(&self) -> u64 {
        self.inner.now_ns()
    }
    fn timeout_ns(&self) -> u64 {
        self.inner.timeout_ns()
    }
}
/// Diagnostic-only cut points park in resident recovery before the harness
/// kills QEMU. A serial marker alone races with later commitment on a fast VM.
pub fn interrupt(point: u8) {
    if selected(point) {
        crate::halt();
    }
}
pub fn selected(point: u8) -> bool {
    let mut selected = [0];
    (unsafe { bexos_secure_monitor::fw_cfg::read(b"opt/bexos/recovery-interrupt", &mut selected) })
        && selected == [b'0' + point]
}
