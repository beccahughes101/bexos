//! Candidate transport uses its own pinned bounce buffer. Provider IPC sessions
//! and their public identities remain untouched while the root stages bytes.
use crate::{kernel_status, monitor_ok, secure_monitor_registers};
use bexos_secure_monitor_abi::firmware as abi;
use bexos_secure_monitor_abi::{MAX_SHARED_BYTES, Request, SHARED_READ, firmware::Call};
use bexos_tee_driver_client::STATUS_INVALID_ARGS;
use bexos_tee_driver_client::{STATUS_TIMED_OUT, STATUS_UNAVAILABLE, STATUS_VERIFY_FAILED};
use bexos_userspace::Memory;

#[derive(Clone, Copy)]
pub struct Report {
    pub outcome: u64,
    pub generation: u64,
    pub component: u64,
    pub slot: u64,
}
fn report(owner: u64) -> Result<Report, i32> {
    let get = |field| {
        secure_monitor_registers(Call::Query { owner, field }.encode()).and_then(monitor_ok)
    };
    // Guest execution between calls can publish a new resident outcome. Never
    // combine the outcome of one transaction with another transaction's image.
    for _ in 0..8 {
        let revision = get(abi::QUERY_REVISION)?;
        let result = Report {
            outcome: get(abi::QUERY_OUTCOME)?,
            generation: get(abi::QUERY_GENERATION)?,
            component: get(abi::QUERY_COMPONENT)?,
            slot: get(abi::QUERY_SLOT)?,
        };
        if get(abi::QUERY_REVISION)? == revision {
            return Ok(result);
        }
    }
    Err(STATUS_UNAVAILABLE)
}
pub fn status() -> Result<Report, i32> {
    let buffer = Buffer::new()?;
    report(buffer.owner)
}
pub fn active_trusty_generation() -> Result<u32, i32> {
    let buffer = Buffer::new()?;
    let value = secure_monitor_registers(
        Call::Query {
            owner: buffer.owner,
            field: abi::QUERY_TRUSTY_GENERATION,
        }
        .encode(),
    )
    .and_then(monitor_ok)?;
    u32::try_from(value).map_err(|_| STATUS_INVALID_ARGS)
}

struct Buffer {
    vmo: u64,
    va: u64,
    pin: u64,
    owner: u64,
}
impl Buffer {
    fn new() -> Result<Self, i32> {
        let mut buffer = Self {
            vmo: Memory::create(MAX_SHARED_BYTES, 0).map_err(kernel_status)?,
            va: 0,
            pin: 0,
            owner: 0,
        };
        buffer.va = Memory::map(buffer.vmo, MAX_SHARED_BYTES, 2 | 4).map_err(kernel_status)?;
        let (physical, pin) = Memory::pin(buffer.vmo).map_err(kernel_status)?;
        buffer.pin = pin;
        buffer.owner = secure_monitor_registers(
            Request::Register {
                address: physical,
                length: MAX_SHARED_BYTES,
                access: SHARED_READ,
            }
            .encode(),
        )
        .and_then(monitor_ok)?;
        Ok(buffer)
    }
}
impl Drop for Buffer {
    fn drop(&mut self) {
        if self.owner != 0 {
            let _ = secure_monitor_registers(Call::Abort { owner: self.owner }.encode());
            let _ = secure_monitor_registers(Request::Unregister { handle: self.owner }.encode());
        }
        if self.pin != 0 {
            let _ = Memory::unpin(self.pin);
        }
        if self.va != 0 {
            let _ = Memory::unmap(self.va, MAX_SHARED_BYTES);
        }
        let _ = Memory::close(self.vmo);
    }
}
pub fn stage_and_activate(
    image: &[u8],
    generation: u64,
    component: u64,
    activation: u64,
    mut progress: impl FnMut() -> Result<(), i32>,
) -> Result<Report, i32> {
    if image.len() as u64 > bexos_secure_monitor_abi::firmware::MAX_BUNDLE_BYTES {
        return Err(STATUS_INVALID_ARGS);
    }
    let buffer = Buffer::new()?;
    let call = |request: Call| secure_monitor_registers(request.encode()).and_then(monitor_ok);
    call(Call::Begin {
        owner: buffer.owner,
        length: image.len() as u64,
        generation,
        component,
    })?;
    for (index, bytes) in image.chunks(MAX_SHARED_BYTES as usize).enumerate() {
        unsafe {
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer.va as *mut u8, bytes.len());
        }
        call(Call::Write {
            owner: buffer.owner,
            offset: index as u64 * MAX_SHARED_BYTES,
            length: bytes.len() as u64,
        })?;
        progress()?;
    }
    if call(Call::Seal {
        owner: buffer.owner,
    })? != generation
    {
        return Err(STATUS_INVALID_ARGS);
    }
    bexos_userspace::syscall::log("tee-driver-trusty: signed firmware snapshot authenticated\n");
    call(Call::Activate {
        owner: buffer.owner,
        mode: activation,
    })?;
    let started = bexos_userspace::live_migration::now_ms();
    loop {
        progress()?;
        let current = report(buffer.owner)?;
        if current.generation != generation || current.component != component {
            return Err(STATUS_UNAVAILABLE);
        }
        match current.outcome {
            abi::COMMITTED | abi::PENDING => return Ok(current),
            abi::ROLLED_BACK | abi::REJECTED => return Err(STATUS_VERIFY_FAILED),
            abi::RECOVERY_REQUIRED => return Err(STATUS_UNAVAILABLE),
            abi::APPLYING | abi::STAGED => (),
            _ => return Err(STATUS_UNAVAILABLE),
        }
        // A timeout is not a rollback claim. The resident owner resolves any
        // mutation already submitted and durable status remains queryable.
        if bexos_userspace::live_migration::now_ms().saturating_sub(started) >= 60_000 {
            return Err(STATUS_TIMED_OUT);
        }
        bexos_userspace::yield_now();
    }
}
