//! Safe ownership wrappers around the restricted-execution syscall transport.
use crate::{Memory, ipc::kernel_call};
use bexos_restricted_abi::STATE_VMO_SIZE;
use kernel_fidl::*;

pub use bexos_restricted_abi::{
    Aarch64StateV1, Architecture, Header, Reason, VectorEntry, X86_64StateV1,
};

pub const PROTOCOL_ID: u64 = 14;

pub struct RestrictedState {
    vmo: u64,
    address: u64,
}

impl RestrictedState {
    pub fn create() -> Result<Self, Status> {
        let vmo = Memory::create(STATE_VMO_SIZE, 0)?;
        match Memory::map(vmo, STATE_VMO_SIZE, 2 | 4) {
            Ok(address) => Ok(Self { vmo, address }),
            Err(error) => {
                let _ = Memory::close(vmo);
                Err(error)
            }
        }
    }

    pub const fn vmo(&self) -> u64 {
        self.vmo
    }

    pub const fn address(&self) -> u64 {
        self.address
    }

    pub fn bind(&self) -> Result<(), Status> {
        let response: RestrictedBindStateResponse = kernel_call(
            PROTOCOL_ID,
            "BindState",
            RESTRICTED_PUBLIC_METHODS,
            &RestrictedBindStateRequest {
                options: 0,
                state_vmo: HandleRef { raw: self.vmo },
            },
        )?;
        crate::ipc::check(response.status)
    }

    pub fn unbind(&self) -> Result<(), Status> {
        unbind()
    }

    /// Enters the register state stored in this object's VMO.
    ///
    /// On success the call never returns normally. `vector_entry` must remain
    /// mapped executable, the calling stack must remain mapped and writable,
    /// and the callback must obey the C ABI and never return.
    pub unsafe fn enter(&self, vector_entry: VectorEntry, context: u64) -> Result<(), Status> {
        unsafe { enter(vector_entry, context) }
    }

    pub fn kick(thread: u64) -> Result<(), Status> {
        kick(thread)
    }
}

pub fn unbind() -> Result<(), Status> {
    let response: RestrictedUnbindStateResponse = kernel_call(
        PROTOCOL_ID,
        "UnbindState",
        RESTRICTED_PUBLIC_METHODS,
        &RestrictedUnbindStateRequest { options: 0 },
    )?;
    crate::ipc::check(response.status)
}

/// Enters a state VMO previously bound to the calling thread.
///
/// The vector must remain executable, the calling stack must remain writable,
/// and the C-ABI callback must never return.
pub unsafe fn enter(vector_entry: VectorEntry, context: u64) -> Result<(), Status> {
    let response: RestrictedEnterResponse = kernel_call(
        PROTOCOL_ID,
        "Enter",
        RESTRICTED_PUBLIC_METHODS,
        &RestrictedEnterRequest {
            options: 0,
            vector_entry: vector_entry as usize as u64,
            context,
        },
    )?;
    crate::ipc::check(response.status)
}

pub fn kick(thread: u64) -> Result<(), Status> {
    let response: RestrictedKickResponse = kernel_call(
        PROTOCOL_ID,
        "Kick",
        RESTRICTED_PUBLIC_METHODS,
        &RestrictedKickRequest {
            thread: HandleRef { raw: thread },
            options: 0,
        },
    )?;
    crate::ipc::check(response.status)
}

impl Drop for RestrictedState {
    fn drop(&mut self) {
        let _ = Memory::unmap(self.address, STATE_VMO_SIZE);
        let _ = Memory::close(self.vmo);
    }
}
