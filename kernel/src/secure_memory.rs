//! Generic memory authority for the x86 monitor. Trusty protocol decoding stays
//! in the provider; the kernel supplies identities for its own live RAM pins.
use bexos_kernel_core::runtime::{Runtime, memory::Backend};
use bexos_secure_monitor_abi::{PINNED_HANDLE_TAG, Request, Status};

pub fn authorize<B: Backend>(
    runtime: &Runtime<B>,
    registers: [u64; 8],
) -> Result<[u64; 8], Status> {
    if let Ok(call) = bexos_secure_monitor_abi::firmware::Call::decode(registers) {
        runtime
            .validate_secure_pin(call.owner() & !PINNED_HANDLE_TAG)
            .map_err(|_| Status::AccessDenied)?;
        return Ok(registers);
    }
    use bexos_secure_monitor_abi::transport::Call;
    if let Ok(call) = Call::decode(registers) {
        let handle = match call {
            Call::Submit { handle, .. } | Call::Poll { handle, .. } => handle,
            Call::Fetch { .. }
            | Call::Complete { .. }
            | Call::RootFetch { .. }
            | Call::RootComplete { .. }
            | Call::IsRegistered { .. } => {
                return Err(Status::AccessDenied);
            }
        };
        runtime
            .validate_secure_pin(handle & !PINNED_HANDLE_TAG)
            .map_err(|_| Status::AccessDenied)?;
        return Ok(registers);
    }
    let request = Request::decode(registers)?;
    match request {
        Request::Register {
            address,
            length,
            access,
        } => {
            let token = runtime
                .secure_pin_for_range(address, length)
                .map_err(|_| Status::AccessDenied)?;
            Ok(Request::RegisterPinned {
                token,
                address,
                length,
                access,
            }
            .encode())
        }
        // This form is synthesized above, never accepted from userspace.
        Request::RegisterPinned { .. } => Err(Status::AccessDenied),
        Request::Unregister { handle } | Request::StageTrustyCore { handle, .. } => {
            if handle & PINNED_HANDLE_TAG == 0 {
                return Err(Status::InvalidHandle);
            }
            runtime
                .validate_secure_pin(handle & !PINNED_HANDLE_TAG)
                .map_err(|_| Status::AccessDenied)?;
            Ok(registers)
        }
        Request::ActivateTrustyCore { .. } => Ok(registers),
    }
}
