//! x86 QL adapter. The provider owns protocol operation IDs; the monitor sees
//! only opaque bounded messages associated with kernel-validated RAM pins.
use bexos_secure_monitor_abi::{Request, SHARED_READ, SHARED_WRITE, Status, transport::Call};
use bexos_tee_driver_client::STATUS_TIMED_OUT;

pub fn register(address: u64, length: u64) -> Result<u64, i32> {
    crate::secure_monitor_registers(
        Request::Register {
            address,
            length,
            access: SHARED_READ | SHARED_WRITE,
        }
        .encode(),
    )
    .and_then(crate::monitor_ok)
}
pub fn unregister(handle: u64) {
    if handle != 0 {
        let _ = crate::secure_monitor_registers(Request::Unregister { handle }.encode());
    }
}
pub fn call(operation: u64, handle: u64, length: u64) -> Result<[u64; 8], i32> {
    let operation =
        u32::try_from(operation).map_err(|_| bexos_tee_driver_client::STATUS_INVALID_ARGS)?;
    let started = bexos_userspace::live_migration::now_ms();
    let mut ticket = None;
    loop {
        let request = match ticket {
            None => Call::Submit {
                handle,
                operation,
                length,
            },
            Some(ticket) => Call::Poll { handle, ticket },
        };
        let output = crate::secure_monitor_registers(request.encode())?;
        match output[0] as i64 {
            0 if ticket.is_none() => ticket = Some(output[1]),
            0 => return Ok([output[1], 0, 0, 0, 0, 0, 0, 0]),
            value if value == Status::Busy as i64 => {}
            value => return Err(crate::monitor_status(value)),
        }
        if bexos_userspace::live_migration::now_ms().saturating_sub(started) >= 30_000 {
            // Fence the abandoned operation's eventual reply. Callers must
            // reconnect a fresh QL device after an indeterminate operation.
            unregister(handle);
            return Err(STATUS_TIMED_OUT);
        }
        bexos_userspace::yield_now();
    }
}
