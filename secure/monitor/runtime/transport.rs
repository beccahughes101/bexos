//! The monitor copies registered bytes; it never decodes Trusty IPC. All
//! accesses run with both domains stopped on the single monitor owner CPU.
#[cfg(not(feature = "resident_nucleus"))]
use bexos_secure_monitor::mailbox::{Mailbox, STATE_BYTES as MAILBOX_STATE_BYTES};
#[cfg(feature = "resident_nucleus")]
use bexos_secure_monitor::transport_lanes::{Lanes as Mailbox, STATE_BYTES as MAILBOX_STATE_BYTES};
use bexos_secure_monitor::{
    shared::{Caller, DomainId, RamWindow, Registry},
    svm::Vmcb,
    vcpu::Registers,
};
use bexos_secure_monitor_abi::{
    MAX_SHARED_BYTES, Request, SHARED_READ, SHARED_WRITE, Status, transport::Call,
};
#[path = "transport_state.rs"]
mod state;
pub use state::{Prepared, STATE_BYTES, prepare_protected, restore_protected, snapshot};
const NORMAL: Caller = Caller {
    domain: DomainId(2),
    may_share: true,
};
static mut REGISTRY: Option<Registry<1, 16>> = None;
static mut MAILBOX: Mailbox = Mailbox::new();
static mut SECURE_BUFFER: u64 = 0;
#[cfg(feature = "resident_nucleus")]
static mut CONTROL_MAILBOX: Mailbox = Mailbox::new();
#[cfg(feature = "resident_nucleus")]
static mut CONTROL_BUFFER: u64 = 0;
#[cfg(feature = "resident_nucleus")]
static mut CANDIDATE_CONTROL_MAILBOX: Mailbox = Mailbox::new();
#[cfg(feature = "resident_nucleus")]
static mut CANDIDATE_CONTROL_BUFFER: u64 = 0;
#[cfg(feature = "resident_nucleus")]
static mut ACTIVE_SECURE_BASE: u64 = crate::BANK;
#[cfg(feature = "resident_nucleus")]
static mut CANDIDATE_SECURE_BASE: u64 = 0;
static mut BOOT_EVIDENCE: bexos_secure_monitor_abi::boot::EvidenceSeal =
    bexos_secure_monitor_abi::boot::EvidenceSeal::empty();

#[cfg(feature = "secure_product")]
pub unsafe fn seal_boot_evidence(digest: [u8; 32]) {
    unsafe {
        (&mut *core::ptr::addr_of_mut!(BOOT_EVIDENCE))
            .install(digest)
            .unwrap();
    }
}
const BOOT_HANDLE: u64 = bexos_secure_monitor_abi::transport::BOOT_OWNER_HANDLE;
static mut BOOT_ACTIVE: bool = false;

fn registry_policy() -> Registry<1, 16> {
    Registry::new([RamWindow {
        owner: NORMAL.domain,
        guest_start: 0,
        host_start: 0x30000000,
        length: 0x30000000,
    }])
    .unwrap()
}
pub unsafe fn initialize() {
    unsafe {
        REGISTRY = Some(registry_policy());
        #[cfg(feature = "resident_nucleus")]
        {
            ACTIVE_SECURE_BASE = crate::BANK;
            CANDIDATE_SECURE_BASE = 0;
            CANDIDATE_CONTROL_MAILBOX = Mailbox::new();
            CANDIDATE_CONTROL_BUFFER = 0;
        }
    }
}

#[cfg(all(feature = "resident_nucleus", feature = "secure_product"))]
pub unsafe fn begin_candidate(bank: u64) {
    unsafe {
        assert!(bank != ACTIVE_SECURE_BASE && CANDIDATE_SECURE_BASE == 0);
        assert!(!(&*core::ptr::addr_of!(CANDIDATE_CONTROL_MAILBOX)).has_running_request());
        CANDIDATE_CONTROL_MAILBOX = Mailbox::new();
        CANDIDATE_CONTROL_BUFFER = 0;
        CANDIDATE_SECURE_BASE = bank;
    }
}

#[cfg(all(feature = "resident_nucleus", feature = "secure_product"))]
pub unsafe fn activate_candidate(bank: u64) {
    unsafe {
        assert!(CANDIDATE_SECURE_BASE == bank);
        ACTIVE_SECURE_BASE = bank;
    }
}

#[cfg(all(feature = "resident_nucleus", feature = "secure_product"))]
pub unsafe fn rollback_candidate(source: u64) {
    unsafe {
        assert!(CANDIDATE_SECURE_BASE != 0);
        ACTIVE_SECURE_BASE = source;
    }
}

#[cfg(all(feature = "resident_nucleus", feature = "secure_product"))]
pub unsafe fn finish_candidate() {
    unsafe {
        (&mut *core::ptr::addr_of_mut!(CANDIDATE_CONTROL_MAILBOX)).revoke(BOOT_HANDLE);
        CANDIDATE_CONTROL_MAILBOX = Mailbox::new();
        CANDIDATE_CONTROL_BUFFER = 0;
        CANDIDATE_SECURE_BASE = 0;
    }
}

#[cfg(any(
    feature = "trusty_recovery_probe",
    all(feature = "resident_nucleus", feature = "secure_product")
))]
pub unsafe fn reset_recovery() {
    unsafe {
        assert!(!BOOT_ACTIVE);
        assert!(!(&*core::ptr::addr_of!(MAILBOX)).has_running_request());
        REGISTRY = Some(registry_policy());
        MAILBOX = Mailbox::new();
        SECURE_BUFFER = 0;
        #[cfg(feature = "resident_nucleus")]
        {
            assert!(!(&*core::ptr::addr_of!(CONTROL_MAILBOX)).has_running_request());
            CONTROL_MAILBOX = Mailbox::new();
            CONTROL_BUFFER = 0;
            CANDIDATE_CONTROL_MAILBOX = Mailbox::new();
            CANDIDATE_CONTROL_BUFFER = 0;
            ACTIVE_SECURE_BASE = crate::BANK;
            CANDIDATE_SECURE_BASE = 0;
        }
        BOOT_EVIDENCE = bexos_secure_monitor_abi::boot::EvidenceSeal::empty();
    }
}

#[cfg(feature = "checkpoint_probe")]
pub unsafe fn has_running_request() -> bool {
    unsafe { (&*core::ptr::addr_of!(MAILBOX)).has_running_request() }
}

#[cfg(feature = "normal_world")]
pub unsafe fn needs_secure_progress() -> bool {
    unsafe {
        #[cfg(feature = "resident_nucleus")]
        if (&*core::ptr::addr_of!(CONTROL_MAILBOX)).needs_secure_progress() {
            return true;
        }
        (&*core::ptr::addr_of!(MAILBOX)).needs_secure_progress()
    }
}

#[cfg(feature = "boot_ipc_probe")]
unsafe fn boot_mailbox(operation: u32, candidate: bool) -> &'static mut Mailbox {
    #[cfg(feature = "resident_nucleus")]
    if operation >= bexos_secure_monitor_abi::transport::BOOT_OWNER_OPERATION_BASE {
        if candidate {
            return unsafe { &mut *core::ptr::addr_of_mut!(CANDIDATE_CONTROL_MAILBOX) };
        }
        return unsafe { &mut *core::ptr::addr_of_mut!(CONTROL_MAILBOX) };
    }
    unsafe { &mut *core::ptr::addr_of_mut!(MAILBOX) }
}

#[cfg(feature = "checkpoint_probe")]
pub unsafe fn discard_for_probe() {
    unsafe {
        REGISTRY = None;
        MAILBOX = Mailbox::new();
        SECURE_BUFFER = 0;
        BOOT_ACTIVE = false;
        BOOT_EVIDENCE = bexos_secure_monitor_abi::boot::EvidenceSeal::empty();
    }
}

unsafe fn request(secure: bool, secure_base: u64, r: [u64; 8]) -> Result<[u64; 8], Status> {
    #[cfg(feature = "resident_nucleus")]
    let candidate = secure
        && unsafe { CANDIDATE_SECURE_BASE } != 0
        && secure_base == unsafe { CANDIDATE_SECURE_BASE };
    #[cfg(not(feature = "resident_nucleus"))]
    let candidate = false;
    if r[1] == bexos_secure_monitor_abi::boot::EVIDENCE_QUERY {
        if secure {
            return Err(Status::AccessDenied);
        }
        unsafe {
            (&*core::ptr::addr_of!(BOOT_EVIDENCE)).confirm(r)?;
        }
        return Ok(Status::Ok.registers(0));
    }
    let registry =
        unsafe { (&mut *core::ptr::addr_of_mut!(REGISTRY)).as_mut() }.ok_or(Status::Unsupported)?;
    #[cfg(feature = "secure_product")]
    if let Ok(call) = bexos_secure_monitor_abi::firmware::Call::decode(r) {
        if secure {
            return Err(Status::AccessDenied);
        }
        return unsafe { crate::replacement::request(call, registry, NORMAL) }
            .map(|value| Status::Ok.registers(value));
    }
    let mailbox = unsafe { &mut *core::ptr::addr_of_mut!(MAILBOX) };
    if let Ok(request) = Request::decode(r) {
        if secure {
            return Err(Status::AccessDenied);
        }
        match request {
            Request::RegisterPinned { .. } => return Ok(registry.dispatch(NORMAL, r)),
            Request::Unregister { handle } => {
                let result = registry.dispatch(NORMAL, r);
                if result[0] == 0 {
                    mailbox.revoke(handle);
                    #[cfg(feature = "secure_product")]
                    unsafe {
                        crate::replacement::revoke(handle);
                    }
                }
                return Ok(result);
            }
            Request::Register { .. } => return Err(Status::AccessDenied),
            _ => return Err(Status::Unsupported),
        }
    }
    match Call::decode(r)? {
        #[cfg(feature = "resident_nucleus")]
        Call::RootFetch { address } if secure => {
            if address == 0
                || address
                    .checked_add(MAX_SHARED_BYTES)
                    .is_none_or(|end| end > crate::BANK_SIZE as u64)
                || (!candidate
                    && unsafe { SECURE_BUFFER } != 0
                    && address < unsafe { SECURE_BUFFER } + MAX_SHARED_BYTES
                    && unsafe { SECURE_BUFFER } < address + MAX_SHARED_BYTES)
            {
                return Err(Status::AccessDenied);
            }
            let destination = unsafe {
                core::slice::from_raw_parts_mut(
                    (secure_base + address) as *mut u8,
                    MAX_SHARED_BYTES as usize,
                )
            };
            let control = unsafe {
                if candidate {
                    &mut *core::ptr::addr_of_mut!(CANDIDATE_CONTROL_MAILBOX)
                } else {
                    &mut *core::ptr::addr_of_mut!(CONTROL_MAILBOX)
                }
            };
            let message = control.fetch(destination)?;
            unsafe {
                if candidate {
                    CANDIDATE_CONTROL_BUFFER = address;
                } else {
                    CONTROL_BUFFER = address;
                }
            }
            Ok([
                0,
                message.ticket,
                message.handle,
                message.operation as u64,
                message.length as u64,
                message.capacity as u64,
                0,
                0,
            ])
        }
        #[cfg(feature = "resident_nucleus")]
        Call::RootComplete { ticket, result } if secure => {
            let address = unsafe {
                if candidate {
                    CANDIDATE_CONTROL_BUFFER
                } else {
                    CONTROL_BUFFER
                }
            };
            if address == 0 {
                return Err(Status::InvalidHandle);
            }
            let response = unsafe {
                core::slice::from_raw_parts(
                    (secure_base + address) as *const u8,
                    MAX_SHARED_BYTES as usize,
                )
            };
            let control = unsafe {
                if candidate {
                    &mut *core::ptr::addr_of_mut!(CANDIDATE_CONTROL_MAILBOX)
                } else {
                    &mut *core::ptr::addr_of_mut!(CONTROL_MAILBOX)
                }
            };
            let completed = control.complete(ticket, result, response);
            if !control.has_running_request() {
                unsafe {
                    if candidate {
                        CANDIDATE_CONTROL_BUFFER = 0;
                    } else {
                        CONTROL_BUFFER = 0;
                    }
                }
            }
            completed?;
            Ok(Status::Ok.registers(0))
        }
        Call::Submit {
            handle,
            operation,
            length,
        } if !secure => {
            let capacity =
                registry.registered_length(NORMAL, handle, SHARED_READ | SHARED_WRITE)?;
            let lease =
                registry.acquire(NORMAL, handle, 0, capacity, SHARED_READ | SHARED_WRITE)?;
            let (host, size) = lease.host_range();
            let snapshot = unsafe { core::slice::from_raw_parts(host as *const u8, size as usize) };
            Ok(Status::Ok.registers(mailbox.submit(
                handle,
                operation,
                length as usize,
                snapshot,
            )?))
        }
        Call::Poll { handle, ticket } if !secure => {
            let capacity = mailbox.capacity(handle, ticket)?;
            let lease = registry.acquire(NORMAL, handle, 0, capacity as u64, SHARED_WRITE)?;
            let (host, size) = lease.host_range();
            let destination =
                unsafe { core::slice::from_raw_parts_mut(host as *mut u8, size as usize) };
            Ok(Status::Ok.registers(mailbox.poll(handle, ticket, destination)? as u64))
        }
        Call::Fetch { address } if secure && !candidate => {
            #[cfg(feature = "resident_nucleus")]
            if unsafe { CONTROL_BUFFER } != 0
                && address < unsafe { CONTROL_BUFFER } + MAX_SHARED_BYTES
                && unsafe { CONTROL_BUFFER } < address.saturating_add(MAX_SHARED_BYTES)
            {
                return Err(Status::AccessDenied);
            }
            if address == 0
                || address
                    .checked_add(MAX_SHARED_BYTES)
                    .is_none_or(|end| end > crate::BANK_SIZE as u64)
            {
                return Err(Status::AccessDenied);
            }
            let destination = unsafe {
                core::slice::from_raw_parts_mut(
                    (secure_base + address) as *mut u8,
                    MAX_SHARED_BYTES as usize,
                )
            };
            let message = mailbox.fetch(destination)?;
            unsafe {
                SECURE_BUFFER = address;
            }
            Ok([
                0,
                message.ticket,
                message.handle,
                message.operation as u64,
                message.length as u64,
                message.capacity as u64,
                0,
                0,
            ])
        }
        Call::Complete { ticket, result } if secure && !candidate => {
            let address = unsafe { SECURE_BUFFER };
            if address == 0 {
                return Err(Status::InvalidHandle);
            }
            let response = unsafe {
                core::slice::from_raw_parts(
                    (secure_base + address) as *const u8,
                    MAX_SHARED_BYTES as usize,
                )
            };
            let completed = mailbox.complete(ticket, result, response);
            if completed.is_ok() {
                unsafe {
                    SECURE_BUFFER = 0;
                }
            }
            completed?;
            Ok(Status::Ok.registers(0))
        }
        Call::IsRegistered { handle } if secure && !candidate => {
            #[cfg(feature = "boot_ipc_probe")]
            if handle == BOOT_HANDLE && unsafe { BOOT_ACTIVE } {
                return Ok(Status::Ok.registers(0));
            }
            registry.registered_length(NORMAL, handle, SHARED_READ | SHARED_WRITE)?;
            Ok(Status::Ok.registers(0))
        }
        _ => Err(Status::AccessDenied),
    }
}

/// Boot owner transport. This never accepts an address or identity from the
/// normal world and does not run normal vCPUs while approval is outstanding.
#[cfg(feature = "boot_ipc_probe")]
pub struct Boot<F: FnMut()> {
    step: F,
    timeout_ns: u64,
    candidate: bool,
}
#[cfg(feature = "boot_ipc_probe")]
impl<F: FnMut()> Boot<F> {
    pub unsafe fn new(step: F) -> Self {
        assert!(!unsafe { BOOT_ACTIVE });
        unsafe {
            BOOT_ACTIVE = true;
        }
        Self {
            step,
            timeout_ns: 30_000_000_000,
            candidate: false,
        }
    }
    #[cfg(all(feature = "resident_nucleus", feature = "secure_product"))]
    pub unsafe fn candidate(step: F) -> Self {
        let mut owner = unsafe { Self::new(step) };
        owner.candidate = true;
        owner
    }
    /// Restricted boot/recovery owner, before normal-world client admission.
    /// This deadline is independent of live preparation and cutover limits.
    pub unsafe fn cold(step: F) -> Self {
        let mut owner = unsafe { Self::new(step) };
        owner.timeout_ns = 600_000_000_000;
        owner
    }
    /// # Safety
    /// The old owner is consumed into the protected handoff. Keep BOOT_ACTIVE
    /// and its Trusty sessions live; no Rust pointer or closure is transferred.
    #[cfg(any(feature = "monitor_transfer", feature = "resident_nucleus"))]
    pub unsafe fn detach(self) {
        let _retained = core::mem::ManuallyDrop::new(self);
    }
    /// # Safety
    /// A validated resident record restored the same transport owner, and the
    /// old owner has been consumed. Do not create a new secure QL session.
    #[cfg(any(feature = "monitor_transfer", feature = "resident_nucleus"))]
    pub unsafe fn resume(step: F) -> Self {
        assert!(unsafe { BOOT_ACTIVE });
        Self {
            step,
            timeout_ns: 30_000_000_000,
            candidate: false,
        }
    }
}
#[cfg(feature = "boot_ipc_probe")]
impl<F: FnMut()> bexos_trusty_boot::ql::Transport for Boot<F> {
    fn exchange(
        &mut self,
        operation: u32,
        length: usize,
        buffer: &mut [u8],
    ) -> Result<i64, bexos_trusty_boot::ql::Error> {
        use bexos_trusty_boot::ql::Error;
        let started = self.now_ns();
        let ticket = unsafe {
            boot_mailbox(operation, self.candidate).submit(BOOT_HANDLE, operation, length, buffer)
        }
        .map_err(|_| Error::Transport)?;
        loop {
            match unsafe {
                boot_mailbox(operation, self.candidate).poll(BOOT_HANDLE, ticket, buffer)
            } {
                Ok(result) => {
                    if operation == bexos_trusty_boot::journal::OPERATION && result < 0 {
                        crate::log("monitor-runtime: journal transport result=\n");
                        crate::hex(result as u64);
                        if buffer.len() >= 12 {
                            crate::log("monitor-runtime: journal response status=\n");
                            crate::hex(u32::from_le_bytes(buffer[8..12].try_into().unwrap()) as u64);
                        }
                    }
                    return Ok(result);
                }
                Err(Status::Busy) => {}
                Err(_) => return Err(Error::Transport),
            }
            if self.now_ns().saturating_sub(started) >= self.timeout_ns {
                unsafe {
                    boot_mailbox(operation, self.candidate).revoke(BOOT_HANDLE);
                }
                return Err(Error::Timeout);
            }
            (self.step)();
        }
    }
    fn now_ns(&self) -> u64 {
        unsafe { bexos_secure_monitor::clock::now_ns() }
    }
    fn timeout_ns(&self) -> u64 {
        self.timeout_ns
    }
}
#[cfg(feature = "boot_ipc_probe")]
impl<F: FnMut()> Drop for Boot<F> {
    fn drop(&mut self) {
        unsafe {
            (&mut *core::ptr::addr_of_mut!(MAILBOX)).revoke(BOOT_HANDLE);
            #[cfg(feature = "resident_nucleus")]
            if self.candidate {
                (&mut *core::ptr::addr_of_mut!(CANDIDATE_CONTROL_MAILBOX)).revoke(BOOT_HANDLE);
            } else {
                (&mut *core::ptr::addr_of_mut!(CONTROL_MAILBOX)).revoke(BOOT_HANDLE);
            }
            BOOT_ACTIVE = false;
        }
    }
}

pub unsafe fn exit(secure: bool, secure_base: u64, vmcb: &mut Vmcb, regs: &mut Registers) {
    let r = [
        vmcb.rax(),
        regs.rbx,
        regs.rcx,
        regs.rdx,
        regs.rsi,
        regs.rdi,
        regs.r8,
        regs.r9,
    ];
    let result = if vmcb.cpl() != 0 {
        Err(Status::AccessDenied)
    } else {
        unsafe { request(secure, secure_base, r) }
    };
    let output = result.unwrap_or_else(|status| status.registers(0));
    vmcb.set_rax(output[0]);
    regs.rbx = output[1];
    regs.rcx = output[2];
    regs.rdx = output[3];
    regs.rsi = output[4];
    regs.rdi = output[5];
    regs.r8 = output[6];
    regs.r9 = output[7];
    vmcb.set_rip(vmcb.rip() + 3);
}
