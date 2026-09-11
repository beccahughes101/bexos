//! Service coordination for shared scanout transport and Flatland fence leases.
use crate::state::Scene;
use bexos_graphics::synchronization::Fences;
use bexos_graphics_runtime::{self as rt, scanout::Client};
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_userspace::{Channel, Memory, live_migration::Resource};
use graphics_fidl::{PresentationRelease, Status};
#[derive(Clone, Copy)]
pub struct Deferred {
    cutoff: u64,
    sequence: u64,
    release: u64,
    failed: bool,
}
pub struct Scanout {
    pub client: Client,
    pub deferred: Vec<Deferred>,
    pub legacy: bool,
    /// Last completed direct presentation. A lease alone may still be pending.
    pub front: u64,
}
impl Default for Scanout {
    fn default() -> Self {
        Self {
            client: Client::default(),
            deferred: Vec::with_capacity(64),
            legacy: false,
            front: 0,
        }
    }
}
impl Scanout {
    pub fn retire(&mut self, sequence: u64, fences: Fences, status: Status) {
        if let Some(acquire) = fences.acquire {
            let _ = Memory::close(acquire);
        }
        if let Some(cutoff) = self.client.lease() {
            self.deferred.push(Deferred {
                cutoff,
                sequence,
                release: fences.release,
                failed: status != Status::Ok,
            });
        } else {
            rt::reply(
                Channel(fences.release),
                &PresentationRelease { sequence, status },
            );
            let _ = Memory::close(fences.release);
        }
    }
    pub fn resources(&self) -> Vec<Resource> {
        let mut out = self.client.resources();
        out.extend(self.deferred.iter().map(|d| Resource::Handle(d.release)));
        out
    }
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        let mut w = Encoder::new();
        w.word(1);
        self.client.encode(&mut w)?;
        w.word(self.front);
        w.word(self.deferred.len() as u64);
        for d in &self.deferred {
            for v in [d.cutoff, d.sequence, d.release, d.failed as u64] {
                w.word(v);
            }
        }
        Ok(w.finish())
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = Decoder::new(bytes);
        if r.word()? != 1 {
            return Err(Error::UnsupportedVersion);
        }
        let client = Client::decode(&mut r)?;
        let mut s = Self {
            client,
            front: r.word()?,
            ..Self::default()
        };
        for _ in 0..r.count(64)? {
            let d = Deferred {
                cutoff: r.word()?,
                sequence: r.word()?,
                release: r.word()?,
                failed: r.flag()?,
            };
            if d.cutoff == 0
                || d.cutoff > s.client.sequence
                || d.sequence == 0
                || d.release == 0
                || s.deferred.iter().any(|v| v.release == d.release)
            {
                return Err(Error::InvalidData);
            }
            s.deferred.push(d);
        }
        r.finish()?;
        if s.front != 0 && !s.client.entries.iter().any(|e| e.key == s.front) {
            return Err(Error::InvalidData);
        }
        Ok(s)
    }
}
pub fn poll(s: &mut Scene, now: u64) {
    let Some(canvas) = s.canvas.as_mut() else {
        return;
    };
    let previous = (s.scanout.client.formats, s.scanout.client.entries.len());
    if s.scanout.client.poll(canvas, now) {
        let current = (s.scanout.client.formats, s.scanout.client.entries.len());
        if previous != current {
            bexos_userspace::log(&format!(
                "scened: scanout control formats={:?} imports={} display={}\n",
                current.0, current.1, canvas.display.0
            ));
        }
        // Retirement can unblock resource destruction without a scene mutation.
        s.dirty = true;
        crate::session::collect_buffers(s);
    }
    let client = &s.scanout.client;
    s.scanout.deferred.retain(|d| {
        if client
            .entries
            .iter()
            .any(|e| e.lease.is_some_and(|l| l.sequence <= d.cutoff))
        {
            return true;
        }
        rt::reply(
            Channel(d.release),
            &PresentationRelease {
                sequence: d.sequence,
                status: if d.failed { Status::ErrIo } else { Status::Ok },
            },
        );
        let _ = Memory::close(d.release);
        false
    });
}
fn immutable(s: &Scene, owner: u64) -> bool {
    s.queues.get(&owner).is_some_and(|queue| {
        s.fences
            .frames
            .get(&(owner, queue.latched_sequence))
            .is_some_and(|f| f.stage == bexos_graphics::synchronization::Stage::Committed)
    })
}
/// True means this frame was submitted, skipped while its immutable buffer is
/// still scanned, or deferred for an import. False requests CPU composition.
pub fn render(s: &mut Scene, now: u64) -> bool {
    if s.scanout.client.busy() {
        return true;
    }
    let Some(c) = s.canvas.as_ref() else {
        return false;
    };
    if s.scanout.client.prune(c).unwrap_or(false) {
        return true;
    }
    let overlays = s.frozen.is_some()
        || s.controls.ring.is_some()
        || s.controls.display != Default::default()
        || s.scanout.deferred.len() >= 48;
    let candidate = bexos_graphics::scanout::candidate(
        s.world.items(&s.snapshots).map(|(_, item)| item),
        c.surface,
        s.scanout.client.formats.unwrap_or((1 << 3) | (1 << 4)),
        overlays,
    );
    let Some(item) = candidate else {
        return false;
    };
    let Some((owner, _)) = s.world.items(&s.snapshots).last() else {
        return false;
    };
    // Legacy Present permits new pixels in the same mapping. Its buffer must
    // be composed/uploaded again, never acknowledged by retaining an old scanout.
    // A committed acquire/release lease makes unchanged direct scanout safe.
    if !immutable(s, owner) {
        return false;
    }
    if s.scanout.client.formats.is_none() {
        return s.scanout.client.discover(c).is_ok();
    }
    let Some(entry) = s
        .scanout
        .client
        .entries
        .iter()
        .find(|e| e.key == item.buffer)
    else {
        return s
            .scanout
            .client
            .import(c, item.buffer, item.surface)
            .is_ok();
    };
    if entry.surface != item.surface {
        return false;
    }
    if entry.lease.is_some() {
        if s.scanout.front == item.buffer {
            let ticks = bexos_userspace::syscall::ticks();
            for queue in s.queues.values_mut() {
                queue.presented_sequence = queue.latched_sequence;
                queue.presented_at = ticks;
            }
            s.dirty = false;
            s.damage = None;
            return true;
        }
        return false;
    }
    match s.scanout.client.present(c, item.buffer) {
        Ok(submission) => {
            s.gpu.invalidate_outputs();
            let frame = crate::presentation::PendingFrame::new(s, submission, now, item.buffer);
            s.pending_frame = Some(frame);
            s.dirty = false;
            s.damage = None;
            true
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retaining_scanout_requires_the_current_committed_buffer_lease() {
        let mut scene = Scene::default();
        scene.queues.entry(1).or_default().latched_sequence = 1;
        assert!(!immutable(&scene, 1));
        scene.fences.insert(1, 1, 2, 3).unwrap();
        assert!(!immutable(&scene, 1));
        scene.fences.signal(1, 1).unwrap();
        assert!(!immutable(&scene, 1));
        scene.fences.commit(1, 1).unwrap();
        assert!(immutable(&scene, 1));
        // A new legacy presentation of the same mapping cannot reuse the
        // previous upload, even while the predecessor's retirement is deferred.
        scene.queues.get_mut(&1).unwrap().latched_sequence = 2;
        assert!(!immutable(&scene, 1));
        scene.fences.commit(1, 2).unwrap();
        assert!(!immutable(&scene, 1));
    }
}
