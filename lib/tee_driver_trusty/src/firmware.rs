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
    pub cutover_started_ms: u64,
}
fn report(owner: u64) -> Result<Report, i32> {
    let get = |field| {
        secure_monitor_registers(Call::Query { owner, field }.encode())
            .and_then(monitor_ok)
            .inspect_err(|status| {
                bexos_userspace::syscall::log(&alloc::format!(
                    "tee-driver-trusty: firmware query field={field} failed status={status}\n"
                ));
            })
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
            cutover_started_ms: 0,
        };
        if get(abi::QUERY_REVISION)? == revision {
            return Ok(result);
        }
    }
    Err(STATUS_UNAVAILABLE)
}
fn stage_error(operation: &str, mapped: i32, status: i32) -> i32 {
    crate::log_status(operation, status);
    if status == STATUS_INVALID_ARGS {
        mapped
    } else {
        status
    }
}
pub fn status() -> Result<Report, i32> {
    let buffer = Buffer::new()?;
    report(buffer.owner)
}
pub fn active_trusty_generation() -> Result<u32, i32> {
    u32::try_from(active_generation(abi::TRUSTY)?).map_err(|_| STATUS_INVALID_ARGS)
}
pub fn active_generation(component: u64) -> Result<u64, i32> {
    let field = match component {
        abi::TRUSTY => abi::QUERY_TRUSTY_GENERATION,
        abi::HYPERVISOR => abi::QUERY_MONITOR_GENERATION,
        _ => return Err(STATUS_INVALID_ARGS),
    };
    let buffer = Buffer::new()?;
    let value = secure_monitor_registers(
        Call::Query {
            owner: buffer.owner,
            field,
        }
        .encode(),
    )
    .and_then(monitor_ok)?;
    Ok(value)
}

/// Holds the kernel-authorized pinned buffer and resident owner handle from
/// Begin through the final durable resolution.  Dropping this value aborts an
/// unresolved transaction through `Buffer::drop`.
pub struct Activation {
    report: Report,
    buffer: Buffer,
}

impl Activation {
    pub fn report(&self) -> Report {
        self.report
    }

    pub fn active_generation(&self, component: u64) -> Result<u64, i32> {
        let field = match component {
            abi::TRUSTY => abi::QUERY_TRUSTY_GENERATION,
            abi::HYPERVISOR => abi::QUERY_MONITOR_GENERATION,
            _ => return Err(STATUS_INVALID_ARGS),
        };
        secure_monitor_registers(
            Call::Query {
                owner: self.buffer.owner,
                field,
            }
            .encode(),
        )
        .and_then(monitor_ok)
    }

    pub fn resolve(&self, commit: bool) -> Result<Report, i32> {
        secure_monitor_registers(
            Call::Resolve {
                owner: self.buffer.owner,
                disposition: if commit {
                    abi::RESOLVE_COMMIT
                } else {
                    abi::RESOLVE_ROLLBACK
                },
            }
            .encode(),
        )
        .and_then(monitor_ok)?;
        report(self.buffer.owner)
    }
}

pub fn resolve(commit: bool) -> Result<Report, i32> {
    let buffer = Buffer::new()?;
    secure_monitor_registers(
        Call::Resolve {
            owner: buffer.owner,
            disposition: if commit {
                abi::RESOLVE_COMMIT
            } else {
                abi::RESOLVE_ROLLBACK
            },
        }
        .encode(),
    )
    .and_then(monitor_ok)?;
    report(buffer.owner)
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
) -> Result<Activation, i32> {
    if image.len() as u64 > bexos_secure_monitor_abi::firmware::MAX_BUNDLE_BYTES {
        return Err(STATUS_INVALID_ARGS);
    }
    let buffer = Buffer::new()?;
    let preparation_started = bexos_userspace::live_migration::now_ms();
    let call = |request: Call| secure_monitor_registers(request.encode()).and_then(monitor_ok);
    let response = secure_monitor_registers(
        Call::Query {
            owner: buffer.owner,
            field: abi::QUERY_CAPABILITIES,
        }
        .encode(),
    )?;
    // Keep the raw owner status here: transport failures, pin failures, and
    // access denial must never be mistaken for an older protocol version.
    use bexos_secure_monitor_abi::Status;
    let capabilities = match response[0] as i64 {
        0 => Ok(response[1]),
        value if value == Status::InvalidArgs as i64 => Err(Status::InvalidArgs),
        value if value == Status::Unsupported as i64 => Err(Status::Unsupported),
        value => return Err(crate::monitor_status(value)),
    };
    abi::activation_preflight(component, activation, capabilities)
        .map_err(|status| crate::monitor_status(status as i64))?;
    call(Call::Begin {
        owner: buffer.owner,
        length: image.len() as u64,
        generation,
        component,
    })
    .map_err(|status| stage_error("firmware begin", crate::STATUS_PEER_CLOSED, status))?;
    for (index, bytes) in image.chunks(MAX_SHARED_BYTES as usize).enumerate() {
        unsafe {
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer.va as *mut u8, bytes.len());
        }
        call(Call::Write {
            owner: buffer.owner,
            offset: index as u64 * MAX_SHARED_BYTES,
            length: bytes.len() as u64,
        })
        .map_err(|status| {
            stage_error(
                "firmware chunk write",
                crate::STATUS_RESOURCE_EXHAUSTED,
                status,
            )
        })?;
        progress()?;
        if bexos_userspace::live_migration::now_ms().saturating_sub(preparation_started) >= 30_000 {
            return Err(STATUS_TIMED_OUT);
        }
    }
    if call(Call::Seal {
        owner: buffer.owner,
    })
    .map_err(|status| stage_error("firmware seal", STATUS_VERIFY_FAILED, status))?
        != generation
    {
        return Err(STATUS_INVALID_ARGS);
    }
    bexos_userspace::syscall::log("tee-driver-trusty: signed firmware snapshot authenticated\n");
    if bexos_userspace::live_migration::now_ms().saturating_sub(preparation_started) >= 30_000 {
        return Err(STATUS_TIMED_OUT);
    }
    let cutover_started = bexos_userspace::live_migration::now_ms();
    let activation_response = secure_monitor_registers(
        Call::Activate {
            owner: buffer.owner,
            mode: activation,
        }
        .encode(),
    )?;
    if activation_response[0] != 0 {
        bexos_userspace::syscall::log(&alloc::format!(
            "tee-driver-trusty: candidate failure registers={activation_response:x?}\n"
        ));
    }
    monitor_ok(activation_response)
        .map_err(|status| stage_error("firmware activate", STATUS_UNAVAILABLE, status))?;
    loop {
        progress()?;
        let current = report(buffer.owner).map_err(|status| {
            stage_error(
                "firmware activation report",
                crate::STATUS_NOT_FOUND,
                status,
            )
        })?;
        if current.generation != generation || current.component != component {
            return Err(STATUS_UNAVAILABLE);
        }
        match current.outcome {
            abi::COMMITTED | abi::PENDING => {
                return Ok(Activation {
                    report: Report {
                        cutover_started_ms: cutover_started,
                        ..current
                    },
                    buffer,
                });
            }
            #[cfg(target_arch = "aarch64")]
            abi::APPLYING if component == abi::TRUSTY && activation == abi::LIVE => {
                return Ok(Activation {
                    report: Report {
                        cutover_started_ms: cutover_started,
                        ..current
                    },
                    buffer,
                });
            }
            abi::ROLLED_BACK | abi::REJECTED => return Err(STATUS_VERIFY_FAILED),
            abi::RECOVERY_REQUIRED => return Err(STATUS_UNAVAILABLE),
            abi::APPLYING | abi::STAGED => (),
            _ => return Err(STATUS_UNAVAILABLE),
        }
        // A timeout is not a rollback claim. The resident owner resolves any
        // mutation already submitted and durable status remains queryable.
        if bexos_userspace::live_migration::now_ms().saturating_sub(cutover_started) >= 150 {
            return Err(STATUS_TIMED_OUT);
        }
        bexos_userspace::yield_now();
    }
}
