//! Userspace access to kernel-enforced ownership transactions.
use crate::ipc::{check, kernel_call};
use kernel_fidl::*;

pub fn set_authority(process: u64, authority: u32) -> Result<(), Status> {
    let r: KernelMigrationControlSetAuthorityResponse = kernel_call(
        7,
        "SetAuthority",
        KERNEL_MIGRATION_CONTROL_PUBLIC_METHODS,
        &KernelMigrationControlSetAuthorityRequest {
            process: HandleRef { raw: process },
            authority,
        },
    )?;
    check(r.status)
}
pub fn begin(
    source: u64,
    target: u64,
    generation: u64,
    preparation_timeout_ms: u32,
    cutover_timeout_ms: u32,
) -> Result<(), Status> {
    let r: KernelMigrationControlBeginResponse = kernel_call(
        7,
        "Begin",
        KERNEL_MIGRATION_CONTROL_PUBLIC_METHODS,
        &KernelMigrationControlBeginRequest {
            source,
            target: HandleRef { raw: target },
            generation,
            preparation_timeout_ms,
            cutover_timeout_ms,
        },
    )?;
    check(r.status)
}
pub fn preserve_handle(source_handle: u64) -> Result<(), Status> {
    let r: KernelMigrationControlPreserveHandleResponse = kernel_call(
        7,
        "PreserveHandle",
        KERNEL_MIGRATION_CONTROL_PUBLIC_METHODS,
        &KernelMigrationControlPreserveHandleRequest { source_handle },
    )?;
    check(r.status)
}
pub fn preserve_mapping(
    source_handle: u64,
    offset: u64,
    vaddr: u64,
    size: u64,
    rights: u32,
) -> Result<(), Status> {
    let r: KernelMigrationControlPreserveMappingResponse = kernel_call(
        7,
        "PreserveMapping",
        KERNEL_MIGRATION_CONTROL_PUBLIC_METHODS,
        &KernelMigrationControlPreserveMappingRequest {
            source_handle,
            offset,
            vaddr,
            size,
            rights,
        },
    )?;
    check(r.status)
}
pub fn preserve_pin(token: u64) -> Result<(), Status> {
    let r: KernelMigrationControlPreservePinResponse = kernel_call(
        7,
        "PreservePin",
        KERNEL_MIGRATION_CONTROL_PUBLIC_METHODS,
        &KernelMigrationControlPreservePinRequest { token },
    )?;
    check(r.status)
}
macro_rules! simple {
    ($name:ident, $method:literal, $request:ident, $response:ident) => {
        pub fn $name() -> Result<(), Status> {
            let r: $response = kernel_call(
                7,
                $method,
                KERNEL_MIGRATION_CONTROL_PUBLIC_METHODS,
                &$request {},
            )?;
            check(r.status)
        }
    };
}
simple!(
    begin_bulk,
    "BeginBulk",
    KernelMigrationControlBeginBulkRequest,
    KernelMigrationControlBeginBulkResponse
);
simple!(
    complete_bulk,
    "CompleteBulk",
    KernelMigrationControlCompleteBulkRequest,
    KernelMigrationControlCompleteBulkResponse
);
simple!(
    commit,
    "Commit",
    KernelMigrationControlCommitRequest,
    KernelMigrationControlCommitResponse
);
simple!(
    abort,
    "Abort",
    KernelMigrationControlAbortRequest,
    KernelMigrationControlAbortResponse
);
pub fn quiesce(final_sequence: u64) -> Result<(), Status> {
    let r: KernelMigrationControlQuiesceResponse = kernel_call(
        7,
        "Quiesce",
        KERNEL_MIGRATION_CONTROL_PUBLIC_METHODS,
        &KernelMigrationControlQuiesceRequest { final_sequence },
    )?;
    check(r.status)
}
pub fn ready(final_sequence: u64) -> Result<(), Status> {
    let r: KernelMigrationControlReadyResponse = kernel_call(
        7,
        "Ready",
        KERNEL_MIGRATION_CONTROL_PUBLIC_METHODS,
        &KernelMigrationControlReadyRequest { final_sequence },
    )?;
    check(r.status)
}

pub fn discard_candidate(process: u64) -> Result<(), Status> {
    let r: KernelMigrationControlDiscardCandidateResponse = kernel_call(
        7,
        "DiscardCandidate",
        KERNEL_MIGRATION_CONTROL_PUBLIC_METHODS,
        &KernelMigrationControlDiscardCandidateRequest {
            process: HandleRef { raw: process },
        },
    )?;
    check(r.status)
}

pub fn begin_cutover(final_sequence: u64) -> Result<(), Status> {
    let r: KernelMigrationControlBeginCutoverResponse = kernel_call(
        7,
        "BeginCutover",
        KERNEL_MIGRATION_CONTROL_PUBLIC_METHODS,
        &KernelMigrationControlBeginCutoverRequest { final_sequence },
    )?;
    check(r.status)
}
