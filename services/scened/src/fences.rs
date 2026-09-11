//! Channel transport for shared acquire/release ownership. All operations are
//! nonblocking; only the front presentation of each session needs polling.
use crate::state::Scene;
use bexos_graphics::synchronization::{Fences, Stage, Synchronization};
use bexos_graphics_runtime as rt;
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_userspace::{Channel, Memory};
use graphics_fidl::*;

fn release(sequence: u64, fences: Fences, status: Status) {
    if let Some(handle) = fences.acquire {
        let _ = Memory::close(handle);
    }
    rt::reply(
        Channel(fences.release),
        &PresentationRelease { sequence, status },
    );
    let _ = Memory::close(fences.release);
}
pub fn cancel(fences: &mut Synchronization, view: u64, sequence: u64) {
    if let Some(frame) = fences.frames.remove(&(view, sequence)) {
        release(sequence, frame, Status::ErrInvalidArgs);
    }
}
pub fn present(s: &mut Scene, channel: Channel, bytes: &[u8], handles: &[u64]) {
    let result = (|| {
        let refs = rt::refs(handles);
        let request = FlatlandSessionPresentWithFencesRequest::decode(bytes, &refs)
            .map_err(|_| Status::ErrInvalidArgs)?;
        if handles.len() != 2
            || handles.iter().any(|h| {
                !Memory::object_info(*h)
                    .is_ok_and(|(kind, _)| kind == kernel_fidl::ObjectType::Channel)
            })
        {
            return Err(Status::ErrInvalidArgs);
        }
        let session = s.sessions.get(&channel.0).ok_or(Status::ErrInvalidArgs)?;
        let target = s.canvas.as_ref().ok_or(Status::ErrIo)?.surface;
        let styled =
            crate::styling::prepare(&mut s.style_cache, channel.0, &session.pending, target)
                .map_err(|_| Status::ErrInvalidArgs)?;
        let graph = s
            .layout_cache
            .entry(channel.0)
            .or_default()
            .prepare(&styled, target)
            .map_err(|_| Status::ErrInvalidArgs)?;
        let queue = s.queues.entry(channel.0).or_default();
        if request.presentation_time_ticks < session.presentation
            || s.fences.frames.len() + s.scanout.deferred.len() >= 64
            || queue.frames.len() == bexos_graphics::presentation::MAX_QUEUED_PRESENTS
        {
            return Err(Status::ErrInvalidArgs);
        }
        let acquire = Memory::duplicate(request.acquire.raw, 1 | 2 | 4 | 32)
            .map_err(|_| Status::ErrInvalidArgs)?;
        let release = match Memory::duplicate(request.release.raw, 1 | 2 | 4 | 32) {
            Ok(handle) => handle,
            Err(_) => {
                let _ = Memory::close(acquire);
                return Err(Status::ErrInvalidArgs);
            }
        };
        let previous = (queue.sequence, queue.last_time);
        let sequence = match queue.submit(&graph, request.presentation_time_ticks) {
            Ok(sequence) => sequence,
            Err(_) => {
                rt::close(&[acquire, release]);
                return Err(Status::ErrInvalidArgs);
            }
        };
        if s.fences
            .insert(channel.0, sequence, acquire, release)
            .is_err()
        {
            queue.frames.pop_back();
            (queue.sequence, queue.last_time) = previous;
            rt::close(&[acquire, release]);
            return Err(Status::ErrInvalidArgs);
        }
        Ok(sequence)
    })();
    rt::reply(
        channel,
        &FlatlandSessionPresentWithFencesResponse {
            status: result.as_ref().map_or_else(|e| *e, |_| Status::Ok),
            sequence: result.unwrap_or(0),
        },
    );
}
pub fn poll(s: &mut Scene) {
    let mut rejected = false;
    for (view, queue) in &mut s.queues {
        let Some(front) = queue.frames.front() else {
            continue;
        };
        let key = (*view, front.sequence);
        let Some(acquire) = s.fences.frames.get(&key).and_then(|f| f.acquire) else {
            continue;
        };
        let mut bytes = [0; 512];
        match rt::stream::read_no_handles(Channel(acquire), &mut bytes) {
            Ok([]) => {
                if let Ok(handle) = s.fences.signal(key.0, key.1) {
                    let _ = Memory::close(handle);
                }
            }
            Err(kernel_fidl::Status::ErrTimedOut) => {}
            _ => {
                queue.reject();
                if let Some(fences) = s.fences.frames.remove(&key) {
                    release(key.1, fences, Status::ErrInvalidArgs);
                }
                rejected = true;
            }
        }
    }
    if rejected {
        crate::session::collect_buffers(s);
    }
}
pub fn commit(s: &mut Scene, view: u64, sequence: u64) {
    if let Ok(Some((sequence, fences))) = s.fences.commit(view, sequence) {
        s.scanout.retire(sequence, fences, Status::Ok);
    }
}
/// The caller removed all scene/queue references before retiring these buffers.
pub fn disconnect(s: &mut Scene, view: u64) {
    s.fences.frames.retain(|(v, sequence), frame| {
        if *v != view {
            return true;
        }
        if frame.stage == Stage::Committed {
            s.scanout.retire(*sequence, *frame, Status::ErrIo);
        } else {
            release(*sequence, *frame, Status::ErrIo);
        }
        false
    });
}
pub fn encode(s: &Synchronization) -> Vec<u8> {
    let mut w = Encoder::new();
    w.word(1);
    w.word(s.frames.len() as u64);
    for ((view, sequence), f) in &s.frames {
        for word in [
            *view,
            *sequence,
            f.acquire.unwrap_or(0),
            f.release,
            match f.stage {
                Stage::Waiting => 0,
                Stage::Ready => 1,
                Stage::Committed => 2,
            },
        ] {
            w.word(word);
        }
    }
    w.finish()
}
pub fn decode(bytes: &[u8]) -> Result<Synchronization, Error> {
    let mut r = Decoder::new(bytes);
    if r.word()? != 1 {
        return Err(Error::UnsupportedVersion);
    }
    let mut state = Synchronization::default();
    for _ in 0..r.count(64)? {
        let key = (r.word()?, r.word()?);
        let acquire = r.word()?;
        let release = r.word()?;
        let stage = match r.word()? {
            0 => Stage::Waiting,
            1 => Stage::Ready,
            2 => Stage::Committed,
            _ => return Err(Error::InvalidData),
        };
        if state
            .frames
            .insert(
                key,
                Fences {
                    acquire: (acquire != 0).then_some(acquire),
                    release,
                    stage,
                },
            )
            .is_some()
        {
            return Err(Error::InvalidData);
        }
    }
    r.finish()?;
    state.validate().map_err(|_| Error::InvalidData)?;
    Ok(state)
}
