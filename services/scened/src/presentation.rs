//! Logical in-flight frame state. Driver completion is not display VSYNC.
use crate::state::Scene;
pub(crate) fn encode_state(s: &Scene) -> Vec<u8> {
    let (outer, inner) = s.presentation_encoding;
    let frame =
        PendingFrame::encode_version(s.pending_frame.as_ref(), if inner == 0 { 3 } else { inner });
    if outer == 1 {
        return frame;
    }
    let mut w = Encoder::new();
    w.word(if outer == 0 { 3 } else { outer });
    w.word(s.takeover_deadline_us);
    w.word(s.takeover_retry_us);
    if outer == 0 || outer >= 3 {
        w.word(s.frame_period_us());
    }
    let mut bytes = w.finish();
    bytes.extend(frame);
    bytes
}
pub(crate) fn decode_state(s: &mut Scene, bytes: &[u8]) -> Result<(), Error> {
    let mut r = Decoder::new(bytes);
    let outer = r.word()?;
    match outer {
        1 => {
            s.pending_frame = PendingFrame::decode(bytes)?;
            s.presentation_encoding = (1, 1);
            s.period_us = 8_333;
            s.takeover_deadline_us = 0;
            s.takeover_retry_us = 0;
        }
        2 | 3 => {
            let deadline = r.word()?;
            let retry = r.word()?;
            if (deadline == 0 && retry != 0) || retry > deadline {
                return Err(Error::InvalidData);
            }
            let period = if outer == 3 { r.word()? } else { 8_333 };
            if ![8_333, 16_667].contains(&period) {
                return Err(Error::InvalidData);
            }
            let offset = if outer == 3 { 32 } else { 24 };
            let frame = PendingFrame::decode(bytes.get(offset..).ok_or(Error::InvalidData)?)?;
            let inner = u64::from_le_bytes(
                bytes
                    .get(offset..offset + 8)
                    .ok_or(Error::InvalidData)?
                    .try_into()
                    .unwrap(),
            );
            s.period_us = period;
            s.presentation_encoding = (outer, inner);
            s.pending_frame = frame;
            s.takeover_deadline_us = deadline;
            s.takeover_retry_us = retry;
        }
        _ => return Err(Error::UnsupportedVersion),
    }
    Ok(())
}
use bexos_graphics_runtime::presentation::Submission;
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};

#[derive(Clone, Debug)]
pub struct PendingFrame {
    pub submission: Submission,
    pub scanout_buffer: u64,
    pub frame_time_us: u64,
    pub timer_deadline_us: u64,
    pub frozen: bool,
    pub resumed: bool,
    pub sequences: [(u64, u64); 16],
    pub count: usize,
}
impl PendingFrame {
    pub fn new(s: &Scene, submission: Submission, now: u64, scanout_buffer: u64) -> Self {
        let mut frame = Self {
            submission,
            scanout_buffer,
            frame_time_us: now,
            timer_deadline_us: s.clock.next_us,
            frozen: s.frozen.is_some(),
            resumed: s.resumed,
            sequences: [(0, 0); 16],
            count: s.queues.len(),
        };
        for (out, (view, queue)) in frame.sequences.iter_mut().zip(&s.queues) {
            *out = (*view, queue.latched_sequence);
        }
        frame
    }

    pub fn encode(frame: Option<&Self>) -> Vec<u8> {
        Self::encode_version(frame, 3)
    }
    pub(crate) fn encode_version(frame: Option<&Self>, version: u64) -> Vec<u8> {
        let mut w = Encoder::new();
        w.word(version);
        w.word(frame.is_some() as u64);
        if let Some(f) = frame {
            if version >= 2 {
                w.word(f.scanout_buffer);
            }
            w.word(f.submission.deadline_us);
            w.word(f.frame_time_us);
            if version >= 3 {
                w.word(f.timer_deadline_us);
            }
            w.word(f.frozen as u64);
            w.word(f.resumed as u64);
            w.word(f.count as u64);
            for (view, sequence) in &f.sequences[..f.count] {
                w.word(*view);
                w.word(*sequence);
            }
        }
        w.finish()
    }
    pub fn decode(bytes: &[u8]) -> Result<Option<Self>, Error> {
        let mut r = Decoder::new(bytes);
        let version = r.word()?;
        if !(1..=3).contains(&version) {
            return Err(Error::UnsupportedVersion);
        }
        let frame = if r.flag()? {
            let scanout_buffer = if version >= 2 { r.word()? } else { 0 };
            let mut f = Self {
                scanout_buffer,
                submission: Submission {
                    deadline_us: r.word()?,
                },
                frame_time_us: r.word()?,
                timer_deadline_us: if version >= 3 { r.word()? } else { 0 },
                frozen: r.flag()?,
                resumed: r.flag()?,
                count: r.count(16)?,
                sequences: [(0, 0); 16],
            };
            if f.submission.deadline_us == 0 || f.frame_time_us > f.submission.deadline_us {
                return Err(Error::InvalidData);
            }
            for i in 0..f.count {
                let pair = (r.word()?, r.word()?);
                if pair.0 == 0 || f.sequences[..i].iter().any(|p| p.0 == pair.0) {
                    return Err(Error::InvalidData);
                }
                f.sequences[i] = pair;
            }
            Some(f)
        } else {
            None
        };
        r.finish()?;
        Ok(frame)
    }
}
pub(crate) fn poll(s: &mut Scene, now: u64) {
    let Some(frame) = s.pending_frame.as_ref() else {
        return;
    };
    let Some(canvas) = s.canvas.as_mut() else {
        return;
    };
    match frame.submission.poll(canvas, now) {
        Ok(None) => return,
        Err(error) => {
            s.pending_frame = None;
            s.metrics.cancel_pending();
            s.metrics.failed = s.metrics.failed.saturating_add(1);
            s.dirty = true;
            s.damage = None;
            bexos_userspace::log(&format!(
                "scened: display submission failed {error:?}; retaining last frame\n"
            ));
            return;
        }
        Ok(Some(_)) => {}
    }
    let frame = s.pending_frame.take().unwrap();
    s.metrics.complete(now, &frame);
    crate::shell::presented(s, frame.frame_time_us, frame.frozen);

    s.scanout.front = frame.scanout_buffer;
    if frame.scanout_buffer != 0 {
        bexos_userspace::log("scened: imported buffer scanout completed\n");
    }
    let ticks = bexos_userspace::syscall::ticks();
    for (view, sequence) in &frame.sequences[..frame.count] {
        if let Some(queue) = s.queues.get_mut(view) {
            queue.presented_sequence = *sequence;
            queue.presented_at = ticks;
        }
    }
    if !frame.frozen {
        if let Some(desktop) = s.desktop.as_mut() {
            if !desktop.presented {
                desktop.presented = true;
                bexos_userspace::log("scened: styled desktop text presented\n");
            }
        }
    }
    if frame.resumed {
        bexos_userspace::log("scened: post-transplant presentation complete\n");
        s.resumed = false;
    }
    if frame.frozen && frame.frame_time_us.saturating_sub(s.start_us) >= 250_000 {
        s.frozen = None;
        s.dirty = true;
        bexos_userspace::log("scened: ready background presented\n");
    }
}
