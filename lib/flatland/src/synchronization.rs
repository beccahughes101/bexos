//! Transport-independent presentation fence ownership and retirement.
use crate::Error;
use alloc::{collections::BTreeMap, vec::Vec};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Waiting,
    Ready,
    Committed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fences {
    pub acquire: Option<u64>,
    pub release: u64,
    pub stage: Stage,
}
#[derive(Default, Debug)]
pub struct Synchronization {
    pub frames: BTreeMap<(u64, u64), Fences>,
}
impl Synchronization {
    pub fn insert(
        &mut self,
        view: u64,
        sequence: u64,
        acquire: u64,
        release: u64,
    ) -> Result<(), Error> {
        if view == 0
            || sequence == 0
            || acquire == 0
            || release == 0
            || acquire == release
            || self.frames.len() >= 64
            || self.frames.contains_key(&(view, sequence))
            || self.frames.values().any(|f| {
                [Some(acquire), Some(release)].contains(&f.acquire)
                    || f.release == acquire
                    || f.release == release
            })
        {
            return Err(Error::Bounds);
        }
        self.frames.insert(
            (view, sequence),
            Fences {
                acquire: Some(acquire),
                release,
                stage: Stage::Waiting,
            },
        );
        Ok(())
    }
    pub fn ready(&self, view: u64, sequence: u64) -> bool {
        self.frames
            .get(&(view, sequence))
            .is_none_or(|f| f.stage != Stage::Waiting)
    }
    /// Returns the consumed acquire capability for the transport owner to close.
    pub fn signal(&mut self, view: u64, sequence: u64) -> Result<u64, Error> {
        let frame = self
            .frames
            .get_mut(&(view, sequence))
            .ok_or(Error::Invalid)?;
        if frame.stage != Stage::Waiting {
            return Err(Error::Stale);
        }
        let handle = frame.acquire.take().ok_or(Error::Invalid)?;
        frame.stage = Stage::Ready;
        Ok(handle)
    }
    /// The caller replaced the committed graph and finished predecessor reads.
    /// Legacy unfenced presents also retire fenced predecessors.
    pub fn commit(&mut self, view: u64, sequence: u64) -> Result<Option<(u64, Fences)>, Error> {
        if !self.ready(view, sequence) {
            return Err(Error::Busy);
        }
        let previous = self
            .frames
            .iter()
            .find(|((v, _), f)| *v == view && f.stage == Stage::Committed)
            .map(|(key, _)| *key);
        if previous.is_some_and(|(_, seq)| seq >= sequence) {
            return Err(Error::Stale);
        }
        let retired = previous.and_then(|key| self.frames.remove(&key).map(|f| (key.1, f)));
        if let Some(frame) = self.frames.get_mut(&(view, sequence)) {
            frame.stage = Stage::Committed;
        }
        Ok(retired)
    }
    pub fn handles(&self) -> Vec<u64> {
        self.frames
            .values()
            .flat_map(|f| f.acquire.into_iter().chain(core::iter::once(f.release)))
            .collect()
    }
    pub fn validate(&self) -> Result<(), Error> {
        if self.frames.len() > 64 {
            return Err(Error::Bounds);
        }
        let mut handles = [0; 128];
        let mut length = 0;
        let mut committed = [0; 64];
        let mut count = 0;
        for ((view, sequence), frame) in &self.frames {
            if *view == 0
                || *sequence == 0
                || frame.release == 0
                || (frame.stage == Stage::Waiting) != frame.acquire.is_some()
            {
                return Err(Error::Invalid);
            }
            for handle in frame
                .acquire
                .into_iter()
                .chain(core::iter::once(frame.release))
            {
                if handle == 0 || handles[..length].contains(&handle) {
                    return Err(Error::Invalid);
                }
                handles[length] = handle;
                length += 1;
            }
            if frame.stage == Stage::Committed {
                if committed[..count].contains(view) {
                    return Err(Error::Invalid);
                }
                committed[count] = *view;
                count += 1;
            }
        }
        Ok(())
    }
}
