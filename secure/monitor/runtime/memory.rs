//! Access domain RAM only while all of that domain's vCPUs are stopped.
use bexos_secure_monitor::{
    guest_memory::{Memory, Paging},
    svm::Vmcb,
};
#[derive(Clone, Copy)]
pub struct DomainMemory {
    pub base: u64,
    pub length: usize,
}
impl DomainMemory {
    unsafe fn memory(&self) -> Memory<'_> {
        Memory::new(unsafe { core::slice::from_raw_parts(self.base as *const u8, self.length) })
    }
    fn paging(vmcb: &Vmcb) -> Option<Paging> {
        if vmcb.cr0() & (1 << 31) == 0 {
            Some(Paging::Disabled)
        } else if vmcb.efer() & (1 << 10) != 0 {
            Some(Paging::Long4 { root: vmcb.cr3() })
        } else {
            None
        }
    }
    pub unsafe fn instruction(&self, vmcb: &Vmcb) -> ([u8; 15], usize) {
        let Some(paging) = Self::paging(vmcb) else {
            return ([0; 15], 0);
        };
        let Some(address) = vmcb.rip().checked_add(vmcb.code_base()) else {
            return ([0; 15], 0);
        };
        unsafe { self.memory() }.instruction(paging, address)
    }
    pub unsafe fn resolve(&self, vmcb: &Vmcb, address: u64) -> Option<u64> {
        unsafe { self.memory() }.resolve(Self::paging(vmcb)?, address)
    }
}
