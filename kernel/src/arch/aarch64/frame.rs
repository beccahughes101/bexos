//! Private AArch64 exception frame; shared policy sees only Context operations.
use bexos_kernel_core::runtime::Context;
#[repr(C, align(16))]
pub(super) struct TrapFrame {
    x: [u64; 31],
    sp_el0: u64,
    pub(super) elr: u64,
    pub(super) spsr: u64,
    q: [u64; 64],
    fpcr: u64,
    fpsr: u64,
    tpidr_el0: u64,
    architecture: u64,
}
impl TrapFrame {
    pub(super) fn view(context: &Context) -> &Self {
        assert!(context.matches_current_architecture());
        unsafe { &*(context as *const Context).cast::<Self>() }
    }
    pub(super) fn view_mut(context: &mut Context) -> &mut Self {
        assert!(context.matches_current_architecture());
        unsafe { &mut *(context as *mut Context).cast::<Self>() }
    }
}
const _: () = assert!(core::mem::size_of::<TrapFrame>() == core::mem::size_of::<Context>());
const _: () = assert!(core::mem::offset_of!(TrapFrame, q) == 272);
const _: () = assert!(core::mem::offset_of!(TrapFrame, architecture) == 808);
