//! The root owns candidate bytes from the first copied chunk. Authentication
//! here is separate from activation and never reports a completed replacement.
use bexos_secure_firmware::{Architecture, Component, Error, MAX_BUNDLE_BYTES, staging::Staging};
use bexos_secure_monitor::{
    image::Image,
    shared::{Caller, Registry},
};
use bexos_secure_monitor_abi::firmware as protocol;
use bexos_secure_monitor_abi::{SHARED_READ, Status, firmware::Call};
#[cfg(feature = "resident_nucleus")]
#[path = "firmware_activation.rs"]
pub mod activation;

#[cfg_attr(
    feature = "resident_nucleus",
    unsafe(link_section = ".resident.firmware")
)]
static mut MEMORY: [u8; MAX_BUNDLE_BYTES] = [0; MAX_BUNDLE_BYTES];
static mut CANDIDATE: Option<Candidate> = None;
struct Candidate {
    owner: u64,
    generation: u64,
    component: Component,
    staging: Staging<'static>,
    activation: Option<u64>,
    sealed: bool,
}
/// # Safety
/// Boot-only access before client admission, with no candidate upload owner.
/// Release this reference before accepting the first firmware hypercall.
#[cfg(feature = "resident_nucleus")]
pub unsafe fn boot_workspace() -> &'static mut [u8] {
    assert!(unsafe { (&*core::ptr::addr_of!(CANDIDATE)).is_none() });
    unsafe { &mut *core::ptr::addr_of_mut!(MEMORY) }
}

pub unsafe fn revoke(owner: u64) {
    let candidate = unsafe { &mut *core::ptr::addr_of_mut!(CANDIDATE) };
    if candidate.as_ref().is_some_and(|c| c.owner == owner) {
        #[cfg(feature = "resident_nucleus")]
        if let Some(c) = candidate.as_ref() {
            activation::record(protocol::REJECTED, c.component, c.generation, 0);
        }
        *candidate = None;
    }
}
pub unsafe fn poll() {
    let candidate = unsafe { &mut *core::ptr::addr_of_mut!(CANDIDATE) };
    let now = unsafe { bexos_secure_monitor::clock::now_ns() };
    if candidate.as_ref().is_some_and(|c| c.staging.expired(now)) {
        #[cfg(feature = "resident_nucleus")]
        if let Some(c) = candidate.as_ref() {
            activation::record(protocol::REJECTED, c.component, c.generation, 0);
        }
        *candidate = None;
        crate::log(
            "monitor-runtime: candidate preparation deadline expired; private snapshot reclaimed\n",
        );
    }
}

/// Both domains are stopped and the kernel has validated the caller's live pin.
pub unsafe fn request(
    call: Call,
    registry: &mut Registry<1, 16>,
    caller: Caller,
) -> Result<u64, Status> {
    let owner = call.owner();
    registry.registered_length(caller, owner, SHARED_READ)?;
    #[cfg(feature = "resident_nucleus")]
    if let Call::Query { field, .. } = call {
        return activation::query(field);
    }
    unsafe {
        poll();
    }
    let state = unsafe { &mut *core::ptr::addr_of_mut!(CANDIDATE) };
    let now = unsafe { bexos_secure_monitor::clock::now_ns() };
    if let Call::Begin {
        length,
        generation,
        component,
        ..
    } = call
    {
        if state.is_some() || {
            #[cfg(feature = "resident_nucleus")]
            {
                activation::busy()
            }
            #[cfg(not(feature = "resident_nucleus"))]
            {
                false
            }
        } {
            return Err(Status::Busy);
        }
        let component = match component {
            bexos_secure_monitor_abi::firmware::TRUSTY => Component::Trusty,
            bexos_secure_monitor_abi::firmware::HYPERVISOR => Component::Hypervisor,
            _ => return Err(Status::InvalidArgs),
        };
        let approved = crate::firmware_generations::approved(component)?;
        if generation <= approved.generation() || generation < approved.floor() {
            return Err(Status::AccessDenied);
        }
        let memory = unsafe { &mut *core::ptr::addr_of_mut!(MEMORY) };
        let staging = Staging::begin(memory, owner, length as usize, now).map_err(status)?;
        *state = Some(Candidate {
            owner,
            generation,
            component,
            staging,
            activation: None,
            sealed: false,
        });
        return Ok(0);
    }
    let candidate = state.as_mut().ok_or(Status::InvalidHandle)?;
    if candidate.owner != owner {
        return Err(Status::AccessDenied);
    }
    match call {
        Call::Write { offset, length, .. } => {
            let lease = registry.acquire(caller, owner, 0, length, SHARED_READ)?;
            let (address, length) = lease.host_range();
            let bytes =
                unsafe { core::slice::from_raw_parts(address as *const u8, length as usize) };
            candidate
                .staging
                .write(owner, offset as usize, bytes, now)
                .map_err(status)?;
            Ok(0)
        }
        Call::Seal { .. } => {
            let result = (|| {
                let bytes = candidate.staging.seal(owner, now).map_err(status)?;
                // The boot owner obtained this component's floor directly
                // from Trusty before sealing boot mutations and entering BexOS.
                let approved = crate::firmware_generations::approved(candidate.component)?;
                let verified = bexos_secure_firmware::verify(
                    bytes,
                    include_bytes!(env!("VERIFIED_ROOT")),
                    Architecture::X86_64,
                    candidate.component,
                    approved.generation(),
                    approved.floor(),
                )
                .map_err(|error| {
                    crate::log(match error {
                        Error::Authentication => {
                            "monitor-runtime: candidate authentication rejected\n"
                        }
                        Error::Policy => "monitor-runtime: candidate policy rejected\n",
                        _ => "monitor-runtime: candidate image format rejected\n",
                    });
                    Status::AccessDenied
                })?;
                if verified.generation != candidate.generation {
                    return Err(Status::AccessDenied);
                }
                let image = Image::parse(
                    verified.image,
                    match candidate.component {
                        Component::Trusty => crate::BANK_SIZE,
                        Component::Hypervisor => crate::BANK as usize,
                    },
                )
                .map_err(|_| Status::InvalidArgs)?;
                if candidate.component == Component::Hypervisor {
                    image
                        .require_identity_linked()
                        .map_err(|_| Status::InvalidArgs)?;
                    #[cfg(feature = "resident_nucleus")]
                    bexos_secure_monitor::policy_image::validate(&image)
                        .map_err(|_| Status::InvalidArgs)?;
                }
                let generation = verified.generation;
                if candidate
                    .staging
                    .expired(unsafe { bexos_secure_monitor::clock::now_ns() })
                {
                    return Err(Status::Busy);
                }
                Ok(generation)
            })();
            if result.is_err() {
                #[cfg(feature = "resident_nucleus")]
                activation::record(
                    protocol::REJECTED,
                    candidate.component,
                    candidate.generation,
                    0,
                );
                *state = None;
            } else {
                candidate.sealed = true;
                #[cfg(feature = "resident_nucleus")]
                activation::record(
                    protocol::STAGED,
                    candidate.component,
                    candidate.generation,
                    0,
                );
                crate::log(
                    "monitor-runtime: signed candidate snapshot authenticated; activation pending\n",
                );
            }
            result
        }
        Call::Abort { .. } => {
            #[cfg(feature = "resident_nucleus")]
            activation::record(
                protocol::REJECTED,
                candidate.component,
                candidate.generation,
                0,
            );
            *state = None;
            Ok(0)
        }
        Call::Activate { mode, .. } => {
            #[cfg(not(feature = "resident_nucleus"))]
            {
                let _ = mode;
                Err(Status::Unsupported)
            }
            #[cfg(feature = "resident_nucleus")]
            {
                if !candidate.sealed {
                    return Err(Status::InvalidArgs);
                }
                let component = match candidate.component {
                    Component::Trusty => protocol::TRUSTY,
                    Component::Hypervisor => protocol::HYPERVISOR,
                };
                let capability =
                    protocol::activation_capability(component, mode).ok_or(Status::InvalidArgs)?;
                if activation::query(protocol::QUERY_CAPABILITIES)? & capability == 0 {
                    return Err(Status::Unsupported);
                }
                if candidate.activation.is_some() {
                    return Err(Status::Busy);
                }
                candidate.activation = Some(mode);
                activation::record(
                    protocol::APPLYING,
                    candidate.component,
                    candidate.generation,
                    0,
                );
                Ok(protocol::APPLYING)
            }
        }
        // Durable resolution is owned by the resident activation path.  An
        // external nucleus cannot safely infer whether a timed-out protected
        // write committed, so it must fail closed instead of replaying it.
        Call::Resolve { .. } => Err(Status::Unsupported),
        Call::Query { .. } => Err(Status::Unsupported),
        Call::Begin { .. } => Err(Status::InvalidArgs),
    }
}
fn status(error: Error) -> Status {
    match error {
        Error::Authentication | Error::Policy | Error::Owner => Status::AccessDenied,
        Error::Deadline | Error::Busy => Status::Busy,
        _ => Status::InvalidArgs,
    }
}
