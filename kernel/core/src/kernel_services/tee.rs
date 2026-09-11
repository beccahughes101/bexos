use super::KernelServiceStatus;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SmcInvocation {
    pub regs: [u64; 8],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SmcResult {
    pub status: KernelServiceStatus,
    pub regs: [u64; 8],
}

pub fn invoke_smc(invocation: SmcInvocation) -> SmcResult {
    if invocation.regs[0] == 0 {
        return SmcResult {
            status: KernelServiceStatus::InvalidArgs,
            regs: [0; 8],
        };
    }

    let _ = invocation;
    SmcResult {
        status: KernelServiceStatus::TimedOut,
        regs: [0xffff_ffff, 0, 0, 0, 0, 0, 0, 0],
    }
}
