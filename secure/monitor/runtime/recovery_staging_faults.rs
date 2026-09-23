//! Native acceptance cut points around the real resident block transport and
//! protected pending publication. This module is absent from product images.
use bexos_secure_firmware::store::{BlockDevice, Error as DiskError};
use bexos_trusty_boot::{
    ql::{Error, Transport},
    selection,
};

pub struct Disk<'a, D> {
    inner: &'a mut D,
    writes: usize,
}
impl<'a, D> Disk<'a, D> {
    pub fn new(inner: &'a mut D) -> Self {
        Self { inner, writes: 0 }
    }
}
impl<D: BlockDevice> BlockDevice for Disk<'_, D> {
    fn architecture(&self) -> bexos_secure_firmware::Architecture {
        self.inner.architecture()
    }
    fn sectors(&self) -> u64 {
        self.inner.sectors()
    }
    fn read(&mut self, sector: u64, bytes: &mut [u8; 512]) -> Result<(), DiskError> {
        self.inner.read(sector, bytes)
    }
    fn write(&mut self, sector: u64, bytes: &[u8; 512]) -> Result<(), DiskError> {
        self.inner.write(sector, bytes)?;
        self.writes += 1;
        if self.writes == 8 && super::selected(3) {
            self.inner.flush()?;
            crate::log("monitor-runtime: interrupted inactive slot has durable partial payload\n");
            super::interrupt(3);
        }
        Ok(())
    }
    fn flush(&mut self) -> Result<(), DiskError> {
        self.inner.flush()
    }
}

pub struct Pending<'a, T>(pub &'a mut T);
impl<T: Transport> Transport for Pending<'_, T> {
    fn exchange(&mut self, operation: u32, length: usize, bytes: &mut [u8]) -> Result<i64, Error> {
        let stage = operation == selection::OPERATION
            && length == 256
            && bytes[8..12] == 2u32.to_le_bytes();
        if stage && super::selected(5) {
            crate::log(
                "monitor-runtime: authenticated inactive slot interrupted before pending publication\n",
            );
            super::interrupt(5);
        }
        let result = self.0.exchange(operation, length, bytes);
        if stage && result == Ok(512) && super::selected(4) {
            crate::log(
                "monitor-runtime: authenticated pending publication acknowledgement withheld\n",
            );
            super::interrupt(4);
        }
        result
    }
    fn now_ns(&self) -> u64 {
        self.0.now_ns()
    }
    fn timeout_ns(&self) -> u64 {
        self.0.timeout_ns()
    }
}
