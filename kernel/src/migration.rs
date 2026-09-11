//! FIDL bridge for the core's resource handover transaction.
use crate::arch::ArchAPI;
use crate::syscall::Rt;
use kernel_fidl::*;

pub fn now_ms() -> u64 {
    crate::arch::CurrentArch::monotonic_ns() / 1_000_000
}

pub fn dispatch(
    rt: &mut Rt,
    ordinal: u64,
    bytes: &[u8],
    handles: &[HandleRef],
    out: &mut [u8],
    oh: &mut [HandleRef],
) -> Result<EncodeResult, FidlWireError> {
    macro_rules! request {
        ($t:ty) => {
            <$t>::decode(bytes, handles)?
        };
    }
    macro_rules! response {
        ($t:ident, $result:expr) => {{
            let status = $result.err().unwrap_or(Status::Ok);
            $t { status }.encode(out, oh)
        }};
    }
    let now = now_ms();
    match ordinal {
        1 => {
            let q = request!(KernelMigrationControlSetAuthorityRequest);
            response!(
                KernelMigrationControlSetAuthorityResponse,
                rt.set_process_authority(q.process.raw, q.authority)
            )
        }
        2 => {
            let q = request!(KernelMigrationControlBeginRequest);
            if crate::transplant::is_pending() {
                return response!(
                    KernelMigrationControlBeginResponse,
                    Err::<(), _>(Status::ErrAlreadyExists)
                );
            }
            response!(
                KernelMigrationControlBeginResponse,
                rt.begin_handover(
                    q.source,
                    q.target.raw,
                    q.generation,
                    now,
                    bexos_migration::Timeouts {
                        preparation_ms: q.preparation_timeout_ms as u64,
                        cutover_ms: q.cutover_timeout_ms as u64
                    }
                )
            )
        }
        3 => {
            let q = request!(KernelMigrationControlPreserveHandleRequest);
            let result = rt.preserve_handle(q.source_handle);
            if let Err(status) = result {
                let cap = q
                    .source_handle
                    .checked_sub(1)
                    .and_then(|index| rt.handles.get(index as usize))
                    .copied()
                    .flatten();
                crate::log_line(&alloc::format!(
                    "service-transplant: preserve rejected handle={} source={:?} capability={cap:?} status={status:?}",
                    q.source_handle,
                    rt.handover.as_ref().map(|h| h.source)
                ));
            }
            response!(KernelMigrationControlPreserveHandleResponse, result)
        }
        4 => {
            let q = request!(KernelMigrationControlPreserveMappingRequest);
            response!(
                KernelMigrationControlPreserveMappingResponse,
                rt.preserve_mapping(q.source_handle, q.offset, q.vaddr, q.size, q.rights)
            )
        }
        5 => {
            let q = request!(KernelMigrationControlPreservePinRequest);
            response!(
                KernelMigrationControlPreservePinResponse,
                rt.preserve_pin(q.token)
            )
        }
        6 => {
            let _ = request!(KernelMigrationControlBeginBulkRequest);
            response!(
                KernelMigrationControlBeginBulkResponse,
                rt.handover_bulk(now)
            )
        }
        7 => {
            let _ = request!(KernelMigrationControlCompleteBulkRequest);
            response!(
                KernelMigrationControlCompleteBulkResponse,
                rt.handover_catch_up(now)
            )
        }
        8 => {
            let q = request!(KernelMigrationControlQuiesceRequest);
            response!(
                KernelMigrationControlQuiesceResponse,
                rt.quiesce_handover(q.final_sequence, now)
            )
        }
        9 => {
            let q = request!(KernelMigrationControlReadyRequest);
            response!(
                KernelMigrationControlReadyResponse,
                rt.ready_handover(q.final_sequence, now)
            )
        }
        10 => {
            let _ = request!(KernelMigrationControlCommitRequest);
            let measurement = rt.handover.as_ref().map(|h| {
                (
                    h.session.generation(),
                    h.session.cutover_started_ms().unwrap_or(now),
                )
            });
            let commit_started = now_ms();
            let result = rt.commit_handover(now);
            let commit_elapsed = now_ms().saturating_sub(commit_started);
            if result.is_ok() {
                crate::log_line(&alloc::format!(
                    "service-transplant: ownership transfer ms={commit_elapsed}"
                ));
                if let Some((generation, start)) = measurement {
                    crate::log_line(&alloc::format!(
                        "service-transplant: committed generation={generation} cutover_ms={}",
                        now_ms().saturating_sub(start)
                    ));
                }
            }
            response!(KernelMigrationControlCommitResponse, result)
        }
        13 => {
            let q = request!(KernelMigrationControlBeginCutoverRequest);
            response!(
                KernelMigrationControlBeginCutoverResponse,
                rt.begin_cutover_handover(q.final_sequence, now)
            )
        }
        12 => {
            let q = request!(KernelMigrationControlDiscardCandidateRequest);
            response!(
                KernelMigrationControlDiscardCandidateResponse,
                rt.discard_candidate(q.process.raw)
            )
        }
        11 => {
            let _ = request!(KernelMigrationControlAbortRequest);
            response!(KernelMigrationControlAbortResponse, rt.abort_handover())
        }
        _ => Err(FidlWireError::UnknownOrdinal(ordinal)),
    }
}
