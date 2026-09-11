//! Authenticated EFI entry and resident evidence confirmation, version 1/x86.
use crate::{HEADER, Status};
pub const EFI_ENTRY_HEADER: u64 = 0x4245584500010002;
pub const EFI_ENTRY_FLAGS: u64 = 3; // Authenticated loader and protected variables.
pub const EVIDENCE_QUERY: u64 = 0x200;
pub fn authenticated_entry(header: u64, flags: u64, reserved: u64) -> bool {
    header == EFI_ENTRY_HEADER && flags == EFI_ENTRY_FLAGS && reserved == 0
}
pub fn evidence_query(digest: [u8; 32]) -> [u64; 8] {
    let mut registers = [HEADER, EVIDENCE_QUERY, 0, 0, 0, 0, 0, 0];
    for (word, bytes) in registers[2..6].iter_mut().zip(digest.chunks_exact(8)) {
        *word = u64::from_le_bytes(bytes.try_into().unwrap());
    }
    registers
}
/// The resident owner installs this only after verification and Trusty approval.
/// Confirmation is unavailable on development and incomplete boot paths.
pub struct EvidenceSeal(Option<[u64; 8]>);
impl EvidenceSeal {
    pub const fn empty() -> Self {
        Self(None)
    }
    pub fn install(&mut self, digest: [u8; 32]) -> Result<(), Status> {
        if self.0.is_some() {
            return Err(Status::Busy);
        }
        self.0 = Some(evidence_query(digest));
        Ok(())
    }
    pub fn confirm(&self, registers: [u64; 8]) -> Result<(), Status> {
        if self.0 == Some(registers) {
            Ok(())
        } else {
            Err(Status::AccessDenied)
        }
    }
    /// Export only to resident protected transition memory.
    pub fn snapshot(&self) -> [u64; 8] {
        self.0.unwrap_or([0; 8])
    }
    /// The resident recovery owner must establish provenance. Shape checking
    /// cannot authenticate an evidence record supplied by either guest.
    pub fn restore_protected(registers: [u64; 8]) -> Result<Self, Status> {
        if registers == [0; 8] {
            return Ok(Self::empty());
        }
        if registers[0] != HEADER || registers[1] != EVIDENCE_QUERY || registers[6..] != [0; 2] {
            return Err(Status::InvalidArgs);
        }
        Ok(Self(Some(registers)))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn entry_contract_requires_exact_architecture_version_and_protection() {
        assert!(authenticated_entry(EFI_ENTRY_HEADER, 3, 0));
        for (header, flags, reserved) in [
            (EFI_ENTRY_HEADER ^ 1, 3, 0),
            (EFI_ENTRY_HEADER ^ (1 << 16), 3, 0),
            (EFI_ENTRY_HEADER, 1, 0),
            (EFI_ENTRY_HEADER, 7, 0),
            (EFI_ENTRY_HEADER, 3, 1),
        ] {
            assert!(!authenticated_entry(header, flags, reserved));
        }
    }
    #[test]
    fn forged_stale_or_incomplete_evidence_never_confirms() {
        let query = evidence_query([0x5a; 32]);
        let mut seal = EvidenceSeal::empty();
        assert_eq!(seal.confirm(query), Err(Status::AccessDenied));
        seal.install([0x5a; 32]).unwrap();
        assert_eq!(seal.confirm(query), Ok(()));
        assert_eq!(
            EvidenceSeal::restore_protected(seal.snapshot())
                .unwrap()
                .confirm(query),
            Ok(())
        );
        assert_eq!(
            EvidenceSeal::restore_protected([0; 8])
                .unwrap()
                .confirm(query),
            Err(Status::AccessDenied)
        );
        for index in 0..8 {
            let mut tampered = query;
            tampered[index] ^= 1;
            assert_eq!(seal.confirm(tampered), Err(Status::AccessDenied));
        }
        assert_eq!(seal.install([0x5b; 32]), Err(Status::Busy));
        assert_eq!(
            seal.confirm(evidence_query([0x5b; 32])),
            Err(Status::AccessDenied)
        );
    }
}
