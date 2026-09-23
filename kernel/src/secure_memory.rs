//! Architecture-neutral memory authority for permanent secure execution
//! owners. Trusty protocol decoding stays in the provider; the kernel supplies
//! identities for its own live RAM pins.
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
    if registers[0] != bexos_secure_monitor_abi::HEADER {
        return authorize_trusty(runtime, registers);
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

const TRUSTY_API_VERSION: u64 = 0xbc00_000b;
const TRUSTY_QL_CREATE: u64 = 0x3200_001e;
const TRUSTY_QL_SHUTDOWN: u64 = 0x3200_001f;
const TRUSTY_QL_COMMAND: u64 = 0x3200_0020;
const TRUSTY_RESTART_LAST: u64 = 0x3c00_0000;
const TRUSTY_RESTART_FIQ: u64 = 0x3c00_0002;
const TRUSTY_NOP: u64 = 0x3c00_0003;
const TRUSTY_MEMORY_ADDRESS_MASK: u64 = 0x0000_ffff_ffff_f000;
const PAGE_BYTES: u64 = 4096;

/// Admit only the legacy calls used by the in-tree Trusty provider. QL-TIPC
/// carries a physical memory descriptor rather than the BexOS handle ABI, so
/// bind it back to a live pin owned by the calling process before execution.
fn authorize_trusty<B: Backend>(
    runtime: &Runtime<B>,
    registers: [u64; 8],
) -> Result<[u64; 8], Status> {
    match registers[0] {
        TRUSTY_QL_CREATE | TRUSTY_QL_SHUTDOWN | TRUSTY_QL_COMMAND
            if registers[4..].iter().all(|value| *value == 0) =>
        {
            let descriptor = registers[1] | (registers[2] << 32);
            let address = descriptor & TRUSTY_MEMORY_ADDRESS_MASK;
            // QL command calls report the occupied byte count, while the pin
            // authority deliberately accepts page ranges only. Authorize the
            // minimal whole-page span containing that command buffer.
            let length = registers[3]
                .checked_add(PAGE_BYTES - 1)
                .map(|value| value & !(PAGE_BYTES - 1))
                .filter(|value| *value != 0)
                .ok_or(Status::InvalidArgs)?;
            runtime
                .secure_pin_for_range(address, length)
                .map_err(|_| Status::AccessDenied)?;
            Ok(registers)
        }
        TRUSTY_API_VERSION
            if registers[1] <= 2 && registers[2..].iter().all(|value| *value == 0) =>
        {
            Ok(registers)
        }
        TRUSTY_RESTART_LAST | TRUSTY_RESTART_FIQ | TRUSTY_NOP
            if registers[1..].iter().all(|value| *value == 0) =>
        {
            Ok(registers)
        }
        TRUSTY_QL_CREATE | TRUSTY_QL_SHUTDOWN | TRUSTY_QL_COMMAND | TRUSTY_API_VERSION
        | TRUSTY_RESTART_LAST | TRUSTY_RESTART_FIQ | TRUSTY_NOP => Err(Status::InvalidArgs),
        _ => Err(Status::Unsupported),
    }
}
