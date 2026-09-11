//! Private normalized trap layout shared only with x86 entry assembly.
use bexos_kernel_core::runtime::Context;
#[repr(C, align(16))]
pub(super) struct TrapFrame {
    syscall_words: [u64; 10],
    pub(super) rax: u64,
    rcx: u64,
    rbx: u64,
    rbp: u64,
    r11: u64,
    reserved: [u64; 15],
    pub(super) cs: u64,
    rsp: u64,
    pub(super) rip: u64,
    pub(super) rflags: u64,
    fxsave: [u64; 64],
    reserved_fp: [u64; 2],
    fs_base: u64,
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
const _: () = assert!(core::mem::offset_of!(TrapFrame, fxsave) == 272);
const _: () = assert!(core::mem::offset_of!(TrapFrame, architecture) == 808);
