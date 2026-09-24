use crate::ipc::{check, kernel_call};
use kernel_fidl::{
    HandleRef, InlineVectorStruct1, Signals, Status, SystemPrivilegedAcknowledgeInterruptRequest,
    SystemPrivilegedAcknowledgeInterruptResponse, SystemPrivilegedMaskInterruptRequest,
    SystemPrivilegedMaskInterruptResponse, TASK_CONTROL_PUBLIC_METHODS, TaskControlWaitManyRequest,
    TaskControlWaitManyResponse,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Interrupt(pub u64);

impl Interrupt {
    pub fn pending(self) -> Result<bool, Status> {
        let items = [InlineVectorStruct1 {
            h: HandleRef { raw: self.0 },
            signals: Signals(Signals::READABLE.0),
        }];
        let response: TaskControlWaitManyResponse = kernel_call(
            3,
            "WaitMany",
            TASK_CONTROL_PUBLIC_METHODS,
            &TaskControlWaitManyRequest {
                items: kernel_fidl::WireVector::from_slice(&items),
                deadline_nanos: 0,
            },
        )?;
        match response.status {
            Status::Ok => Ok(response.observed_signals.0 & Signals::READABLE.0 != 0),
            Status::ErrTimedOut => Ok(false),
            status => Err(status),
        }
    }

    pub fn acknowledge(self) -> Result<(), Status> {
        let response: SystemPrivilegedAcknowledgeInterruptResponse = kernel_call(
            4,
            "AcknowledgeInterrupt",
            kernel_fidl::SYSTEM_PRIVILEGED_BEXOS_SYSTEM_PRIVILEGED_METHODS,
            &SystemPrivilegedAcknowledgeInterruptRequest {
                irq_handle: HandleRef { raw: self.0 },
            },
        )?;
        check(response.status)
    }

    pub fn set_masked(self, masked: bool) -> Result<(), Status> {
        let response: SystemPrivilegedMaskInterruptResponse = kernel_call(
            4,
            "MaskInterrupt",
            kernel_fidl::SYSTEM_PRIVILEGED_BEXOS_SYSTEM_PRIVILEGED_METHODS,
            &SystemPrivilegedMaskInterruptRequest {
                irq_handle: HandleRef { raw: self.0 },
                masked,
            },
        )?;
        check(response.status)
    }
}
