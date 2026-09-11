//! Root transport state travels with both stopped domains. Never accept these
//! bytes through a guest hypercall: their authority is resident-memory origin.
use super::*;
use bexos_secure_monitor_abi::boot::EvidenceSeal;
use sha2::{Digest, Sha256};

const REGISTRY_END: usize = 128 + Registry::<1, 16>::state_bytes();
const CONTROL_AT: usize = REGISTRY_END + MAILBOX_STATE_BYTES;
const END: usize = CONTROL_AT
    + if cfg!(feature = "resident_nucleus") {
        MAILBOX_STATE_BYTES
    } else {
        0
    };
pub const STATE_BYTES: usize = END + 32;
const MAGIC: &[u8; 8] = if cfg!(feature = "resident_nucleus") {
    b"BEXTP002"
} else {
    b"BEXTP001"
};

/// Decoded owner-private state. Dropping preparation leaves live transport
/// untouched; installation is infallible after all domain records validate.
pub struct Prepared {
    registry: Registry<1, 16>,
    mailbox: Mailbox,
    #[cfg(feature = "resident_nucleus")]
    control: Mailbox,
    #[cfg(feature = "resident_nucleus")]
    control_address: u64,
    evidence: EvidenceSeal,
    address: u64,
    boot_active: bool,
}
impl Prepared {
    /// Both domains must remain stopped from preparation through installation.
    pub unsafe fn install(self) {
        unsafe {
            REGISTRY = Some(self.registry);
            MAILBOX = self.mailbox;
            BOOT_EVIDENCE = self.evidence;
            SECURE_BUFFER = self.address;
            BOOT_ACTIVE = self.boot_active;
            #[cfg(feature = "resident_nucleus")]
            {
                CONTROL_MAILBOX = self.control;
                CONTROL_BUFFER = self.control_address;
            }
        }
    }
}

pub unsafe fn snapshot(output: &mut [u8]) -> Result<(), Status> {
    if output.len() != STATE_BYTES {
        return Err(Status::InvalidArgs);
    }
    output.fill(0);
    output[..8].copy_from_slice(MAGIC);
    output[8..16].copy_from_slice(&bexos_secure_monitor_abi::HEADER.to_le_bytes());
    unsafe {
        output[16..24].copy_from_slice(&SECURE_BUFFER.to_le_bytes());
        output[24] = u8::from(BOOT_ACTIVE);
        #[cfg(feature = "resident_nucleus")]
        output[32..40].copy_from_slice(&CONTROL_BUFFER.to_le_bytes());
        for (target, word) in output[64..128]
            .chunks_exact_mut(8)
            .zip((&*core::ptr::addr_of!(BOOT_EVIDENCE)).snapshot())
        {
            target.copy_from_slice(&word.to_le_bytes());
        }
        (&*core::ptr::addr_of!(REGISTRY))
            .as_ref()
            .ok_or(Status::Unsupported)?
            .snapshot(&mut output[128..REGISTRY_END])?;
        (&*core::ptr::addr_of!(MAILBOX)).snapshot(2, &mut output[REGISTRY_END..CONTROL_AT])?;
        #[cfg(feature = "resident_nucleus")]
        (&*core::ptr::addr_of!(CONTROL_MAILBOX)).snapshot(1, &mut output[CONTROL_AT..END])?;
    }
    let digest = Sha256::digest(&output[..END]);
    output[END..].copy_from_slice(&digest);
    Ok(())
}

/// Validate every nested record before installing any part of the handoff.
/// Guest memory and vCPU state, including a running worker, must be retained.
pub unsafe fn restore_protected(input: &[u8]) -> Result<(), Status> {
    unsafe {
        prepare_protected(input)?.install();
    }
    Ok(())
}

/// Decode protected state without changing the active transport. This permits
/// the complete two-domain handoff to validate before either owner changes.
pub unsafe fn prepare_protected(input: &[u8]) -> Result<Prepared, Status> {
    if input.len() != STATE_BYTES {
        return Err(Status::InvalidArgs);
    }
    if &input[..8] != MAGIC
        || input[8..16] != bexos_secure_monitor_abi::HEADER.to_le_bytes()
        || input[24] > 1
        || input[25..32] != [0; 7]
        || input[40..64] != [0; 24]
        || !cfg!(feature = "resident_nucleus") && input[32..40] != [0; 8]
        || input[END..] != Sha256::digest(&input[..END])[..]
    {
        return Err(Status::AccessDenied);
    }
    let address = u64::from_le_bytes(input[16..24].try_into().unwrap());
    if address != 0
        && address
            .checked_add(MAX_SHARED_BYTES)
            .is_none_or(|end| end > crate::BANK_SIZE as u64)
    {
        return Err(Status::AccessDenied);
    }
    let boot_active = input[24] == 1;
    #[cfg(feature = "resident_nucleus")]
    let control_address = u64::from_le_bytes(input[32..40].try_into().unwrap());
    #[cfg(feature = "resident_nucleus")]
    if control_address != 0
        && (control_address
            .checked_add(MAX_SHARED_BYTES)
            .is_none_or(|end| end > crate::BANK_SIZE as u64)
            || address != 0
                && control_address < address + MAX_SHARED_BYTES
                && address < control_address + MAX_SHARED_BYTES)
    {
        return Err(Status::AccessDenied);
    }
    if boot_active && !cfg!(feature = "boot_ipc_probe") {
        return Err(Status::AccessDenied);
    }
    let mut evidence = [0; 8];
    for (word, bytes) in evidence.iter_mut().zip(input[64..128].chunks_exact(8)) {
        *word = u64::from_le_bytes(bytes.try_into().unwrap());
    }
    if cfg!(feature = "secure_product") && evidence == [0; 8] {
        return Err(Status::AccessDenied);
    }
    let evidence = EvidenceSeal::restore_protected(evidence)?;
    let mut registry = registry_policy();
    registry.restore_protected(&input[128..REGISTRY_END])?;
    let mut registered = |handle, capacity| {
        if handle == BOOT_HANDLE {
            return boot_active && capacity > 0 && capacity <= MAX_SHARED_BYTES as usize;
        }
        registry.registered_length(NORMAL, handle, SHARED_READ | SHARED_WRITE)
            == Ok(capacity as u64)
    };
    let mailbox = &input[REGISTRY_END..CONTROL_AT];
    if Mailbox::validate_protected(2, mailbox, &mut registered)? && address == 0 {
        return Err(Status::InvalidArgs);
    }
    // A revoked running request may still have its old secure completion
    // buffer. Keep it even if its mailbox is empty or a later request queued.
    let mut restored = Mailbox::new();
    restored.restore_protected(2, mailbox, registered)?;
    #[cfg(feature = "resident_nucleus")]
    let control = {
        let mut control = Mailbox::new();
        control.restore_protected(1, &input[CONTROL_AT..END], |handle, capacity| {
            handle == BOOT_HANDLE
                && boot_active
                && capacity > 0
                && capacity <= MAX_SHARED_BYTES as usize
        })?;
        if control.has_running_request() && control_address == 0 {
            return Err(Status::InvalidArgs);
        }
        control
    };
    Ok(Prepared {
        registry,
        mailbox: restored,
        evidence,
        address,
        boot_active,
        #[cfg(feature = "resident_nucleus")]
        control,
        #[cfg(feature = "resident_nucleus")]
        control_address,
    })
}
