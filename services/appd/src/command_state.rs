//! Identity and process-control ownership retained across appd replacement.
use crate::OpenerBinding;
use alloc::{string::String, vec::Vec};
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};

pub const LIMIT: usize = 64;
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ControlledProcess {
    pub channel: u64,
    pub process: u64,
    pub package: String,
    pub uid: u64,
    pub watch_pending: bool,
    pub completion: Option<i32>,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CommandState {
    pub bindings: Vec<OpenerBinding>,
    pub processes: Vec<ControlledProcess>,
}
impl CommandState {
    pub fn handles(&self) -> impl Iterator<Item = u64> + '_ {
        self.bindings.iter().map(|b| b.channel).chain(
            self.processes
                .iter()
                .flat_map(|p| [p.channel, p.process])
                .filter(|h| *h != 0),
        )
    }
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Encoder::new();
        w.word(1);
        w.word(self.bindings.len() as u64);
        for b in &self.bindings {
            w.word(b.channel);
            w.text(&b.package);
            w.word(b.uid);
            w.word(b.system as u64);
        }
        w.word(self.processes.len() as u64);
        for p in &self.processes {
            w.word(p.channel);
            w.word(p.process);
            w.text(&p.package);
            w.word(p.uid);
            w.word(p.watch_pending as u64);
            w.word(p.completion.is_some() as u64);
            w.word(p.completion.unwrap_or(0) as i64 as u64);
        }
        w.finish()
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = Decoder::new(bytes);
        if r.word()? != 1 {
            return Err(Error::InvalidData);
        }
        let mut state = Self::default();
        for _ in 0..r.count(LIMIT)? {
            let channel = r.word()?;
            let package = r.text(128)?.into();
            let uid = r.word()?;
            let system = r.flag()?;
            if channel == 0 || system != (uid == 0) {
                return Err(Error::InvalidData);
            }
            state.bindings.push(OpenerBinding {
                channel,
                package,
                uid,
                system,
            });
        }
        for _ in 0..r.count(LIMIT)? {
            let channel = r.word()?;
            let process = r.word()?;
            let package = r.text(128)?.into();
            let uid = r.word()?;
            let watch_pending = r.flag()?;
            let completed = r.flag()?;
            let code = r.word()? as i64;
            if (channel == 0 && watch_pending)
                || process == 0
                || code != code as i32 as i64
                || (!completed && code != 0)
            {
                return Err(Error::InvalidData);
            }
            state.processes.push(ControlledProcess {
                channel,
                process,
                package,
                uid,
                watch_pending,
                completion: completed.then_some(code as i32),
            });
        }
        r.finish()?;
        let mut handles: Vec<_> = state.handles().collect();
        handles.sort_unstable();
        if handles.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(Error::InvalidData);
        }
        Ok(state)
    }
}
