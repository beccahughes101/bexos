use super::{HANDOFF_ADDR, PENDING, UPDATE_STATUS, prepare};
use crate::arch::ArchAPI;
use crate::{memory::Frames, state::Global};
use bexos_kernel_core::{
    runtime::incremental::BulkCursor,
    transplant::{KernelTransplantHandoff, checksum, codec::Writer},
};
use bexos_migration::{Phase, Session};
use core::sync::atomic::Ordering;

struct Live {
    session: Session,
    cursor: BulkCursor,
    frame_index: usize,
    sequence: u64,
    digest: u64,
    bulk_sequence: u64,
    bulk_digest: u64,
    catchup_turns: u32,
}
static LIVE: Global<Option<Live>> = Global::new(None);
const PACKET: u64 = bexos_boot::UPDATE_SNAPSHOT + 4096;
pub fn phase() -> Option<Phase> {
    LIVE.with(|slot| slot.as_ref().map(|s| s.session.phase()))
}

pub fn begin() -> Result<(), ()> {
    let handoff = unsafe { &*(HANDOFF_ADDR as *const KernelTransplantHandoff) };
    prepare::call(0, handoff.generation, 0)?;
    let mut session = Session::new(
        handoff.generation,
        crate::migration::now_ms(),
        Default::default(),
    )
    .map_err(|_| ())?;
    session
        .begin_bulk(crate::migration::now_ms())
        .map_err(|_| ())?;
    let cursor = crate::userspace::RUNTIME.with(|slot| {
        let rt = slot.as_mut().unwrap();
        let cursor = rt.begin_live_snapshot(16384).map_err(|_| ())?;
        rt.backend.frames.begin_live_snapshot();
        Ok::<_, ()>(cursor)
    })?;
    LIVE.with(|slot| {
        *slot = Some(Live {
            session,
            cursor,
            frame_index: 0,
            sequence: 0,
            digest: 0,
            bulk_sequence: 0,
            bulk_digest: 0,
            catchup_turns: 0,
        })
    });
    crate::log_line("heart-transplant: live bulk sync started; userspace remains scheduled");
    Ok(())
}

fn send(state: &mut Live, key: u64) -> Result<(), ()> {
    let bytes = unsafe { core::slice::from_raw_parts_mut(PACKET as *mut u8, 32768) };
    let mut w = Writer::new(&mut bytes[..32760]);
    let next = state.sequence.checked_add(1).ok_or(())?;
    w.word(next).map_err(|_| ())?;
    crate::userspace::RUNTIME.with(|slot| {
        let rt = slot.as_mut().unwrap();
        if key >> 56 == prepare::FRAME_RECORD >> 56 {
            let index = (key & 0x00ff_ffff_ffff_ffff) as usize;
            w.word(key).map_err(|_| ())?;
            w.word(rt.backend.frames.word(index).ok_or(())?)
                .map_err(|_| ())?;
            rt.backend.frames.copied(index);
        } else {
            rt.write_record(key, &mut w).map_err(|_| ())?;
            rt.record_copied(key);
        }
        Ok::<_, ()>(())
    })?;
    let len = w.len();
    let hash = checksum(&bytes[..len]) as u64;
    bytes[len..len + 8].copy_from_slice(&hash.to_le_bytes());
    prepare::call(1, PACKET, (len + 8) as u64).map_err(|()| {
        crate::log_line(&alloc::format!(
            "heart-transplant: rejected runtime record key={key:#x} sequence={next}"
        ));
    })?;
    state.sequence = next;
    state.digest = state.digest.rotate_left(7) ^ hash;
    Ok(())
}

fn dirty() -> Result<Option<u64>, ()> {
    crate::userspace::RUNTIME.with(|slot| {
        let rt = slot.as_ref().unwrap();
        if let Some(index) = rt.backend.frames.dirty_next() {
            return Ok(Some(prepare::FRAME_RECORD | index as u64));
        }
        rt.dirty_next().map_err(|_| ())
    })
}

/// Called after saving/scheduling a userspace context, never with the runtime
/// borrowed. Each turn copies a bounded number of records and returns to EL0.
pub fn step() {
    let result = LIVE.with(|slot| {
        let Some(s) = slot.as_mut() else {
            return Ok(false);
        };
        s.session.poll(crate::migration::now_ms()).map_err(|_| ())?;
        if s.session.phase() == Phase::Bulk {
            for _ in 0..8 {
                let next = if s.frame_index < Frames::SNAPSHOT_WORDS {
                    let next = prepare::FRAME_RECORD | s.frame_index as u64;
                    s.frame_index += 1;
                    Some(next)
                } else {
                    s.cursor.next()
                };
                if let Some(key) = next {
                    send(s, key)?;
                } else {
                    s.session
                        .bulk_complete(crate::migration::now_ms())
                        .map_err(|_| ())?;
                    s.bulk_sequence = s.sequence;
                    s.bulk_digest = s.digest;
                    crate::log_line(
                        "heart-transplant: live bulk sync complete; catching up mutations",
                    );
                    break;
                }
            }
            return Ok(false);
        }
        s.catchup_turns += 1;
        for _ in 0..32 {
            let Some(key) = dirty()? else {
                return Ok(s.catchup_turns > 1);
            };
            send(s, key)?;
        }
        Ok(false)
    });
    match result {
        Ok(true) => finish(),
        Ok(false) => {}
        Err(()) => abort(),
    }
}

fn finish() {
    let result = LIVE.with(|slot| {
        let s = slot.as_mut().ok_or(())?;
        s.session
            .quiesce(s.sequence, crate::migration::now_ms())
            .map_err(|_| ())?;
        // No EL0 execution occurs between the last delta and this validation.
        prepare::call(2, 0, 0)?;
        s.session
            .ready(s.sequence, crate::migration::now_ms())
            .map_err(|_| ())?;
        let bytes =
            unsafe { core::slice::from_raw_parts_mut(bexos_boot::UPDATE_SNAPSHOT as *mut u8, 64) };
        let mut w = Writer::new(bytes);
        for n in [
            prepare::RECEIPT_MAGIC,
            s.session.generation(),
            s.bulk_sequence,
            s.bulk_digest,
            prepare::RECEIPT_MAGIC,
            s.session.generation(),
            s.sequence,
            s.digest,
        ] {
            w.word(n).map_err(|_| ())?;
        }
        let h = unsafe { &mut *(HANDOFF_ADDR as *mut KernelTransplantHandoff) };
        h.snapshot.len = 64;
        h.snapshot_checksum = checksum(bytes) as u64;
        h.cpu = super::capture_cpu_context();
        h.system = crate::arch::CurrentArch::capture_system();
        super::orchestrator::authorize(h, &bytes[..32], &bytes[32..]).map_err(|_| ())?;
        h.switch_authorized = 1;
        h.seal();
        h.validate_snapshot(bytes).map_err(|_| ())?;
        s.session
            .commit(crate::migration::now_ms())
            .map_err(|_| ())?;
        crate::log_line(&alloc::format!(
            "heart-transplant: precommit cutover_ms={}",
            crate::migration::now_ms().saturating_sub(s.session.cutover_started_ms().unwrap())
        ));
        Ok::<_, ()>(())
    });
    if result.is_err() {
        abort();
        return;
    }
    UPDATE_STATUS.store(3, Ordering::Release);
    crate::log_line("heart-transplant: SWITCH snapshot sealed; jumping to replacement");
    let entry = unsafe { (*(HANDOFF_ADDR as *const KernelTransplantHandoff)).replacement_entry };
    crate::arch::CurrentArch::jump_to_replacement(entry, HANDOFF_ADDR)
}

pub fn abort() {
    LIVE.with(|slot| *slot = None);
    crate::userspace::RUNTIME.with(|slot| {
        let rt = slot.as_mut().unwrap();
        rt.end_live_snapshot();
        rt.backend.frames.end_live_snapshot();
        crate::arch::CurrentArch::abort_replacement(rt);
    });
    PENDING.store(0, Ordering::Release);
    UPDATE_STATUS.store(5, Ordering::Release);
    crate::log_line("heart-transplant: live preparation aborted; old kernel still serving");
}
