use bexos_migration::{
    Error as MigrationError,
    codec::{Decoder, Encoder},
};
use starnix_kernel::{EAGAIN, EINVAL};
use std::vec::Vec;

pub const MAX_SIGNAL: u32 = 64;
pub const SIGKILL: u32 = 9;
pub const SIGSTOP: u32 = 19;
pub const SA_RESTORER: u64 = 0x0400_0000;
pub const SA_ONSTACK: u64 = 0x0800_0000;
pub const SA_NODEFER: u64 = 0x4000_0000;
pub const SA_RESETHAND: u64 = 0x8000_0000;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SignalAction {
    pub handler: u64,
    pub flags: u64,
    pub restorer: u64,
    pub mask: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AltStack {
    pub address: u64,
    pub size: u64,
    pub flags: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SignalState {
    actions: [SignalAction; 65],
    mask: u64,
    pending: u64,
    alt_stack: AltStack,
    saved_mask: Option<u64>,
}

impl Default for SignalState {
    fn default() -> Self {
        Self {
            actions: [SignalAction::default(); 65],
            mask: 0,
            pending: 0,
            alt_stack: AltStack::default(),
            saved_mask: None,
        }
    }
}

fn bit(signal: u32) -> Result<u64, i64> {
    if signal == 0 || signal > MAX_SIGNAL {
        Err(EINVAL)
    } else {
        Ok(1 << (signal - 1))
    }
}

impl SignalState {
    pub fn action(&self, signal: u32) -> Result<SignalAction, i64> {
        bit(signal)?;
        Ok(self.actions[signal as usize])
    }

    pub fn set_action(&mut self, signal: u32, action: SignalAction) -> Result<SignalAction, i64> {
        bit(signal)?;
        if matches!(signal, SIGKILL | SIGSTOP) {
            return Err(EINVAL);
        }
        let previous = self.actions[signal as usize];
        self.actions[signal as usize] = action;
        Ok(previous)
    }

    pub fn mask(&self) -> u64 {
        self.mask
    }

    pub fn update_mask(&mut self, how: u32, value: u64) -> Result<u64, i64> {
        let previous = self.mask;
        let immutable = bit(SIGKILL).unwrap() | bit(SIGSTOP).unwrap();
        self.mask = match how {
            0 => self.mask | value,
            1 => self.mask & !value,
            2 => value,
            _ => return Err(EINVAL),
        } & !immutable;
        Ok(previous)
    }

    pub fn suspend(&mut self, temporary_mask: u64) {
        self.saved_mask = Some(self.mask);
        self.mask = temporary_mask & !(bit(SIGKILL).unwrap() | bit(SIGSTOP).unwrap());
    }

    pub fn queue(&mut self, signal: u32) -> Result<(), i64> {
        self.pending |= bit(signal)?;
        Ok(())
    }

    pub fn next(&mut self) -> Option<(u32, SignalAction, u64)> {
        let deliverable = self.pending & !self.mask;
        if deliverable == 0 {
            return None;
        }
        let signal = deliverable.trailing_zeros() + 1;
        self.pending &= !bit(signal).unwrap();
        let action = self.actions[signal as usize];
        let old_mask = self.saved_mask.take().unwrap_or(self.mask);
        self.mask = old_mask;
        if action.flags & SA_NODEFER == 0 {
            self.mask |= bit(signal).unwrap();
        }
        self.mask |= action.mask;
        if action.flags & SA_RESETHAND != 0 {
            self.actions[signal as usize] = SignalAction::default();
        }
        Some((signal, action, old_mask))
    }

    pub fn restore_mask(&mut self, mask: u64) {
        self.mask = mask;
    }
    pub fn alt_stack(&self) -> AltStack {
        self.alt_stack
    }

    pub fn set_alt_stack(&mut self, stack: AltStack) -> Result<AltStack, i64> {
        if stack.flags & !2 != 0 || (stack.flags == 0 && stack.size < 2048) {
            return Err(EINVAL);
        }
        let previous = self.alt_stack;
        self.alt_stack = stack;
        Ok(previous)
    }

    pub fn validate_target(&self, pid: i64, tid: i64) -> Result<(), i64> {
        if matches!(pid, 0 | 1) && matches!(tid, 0 | 1) {
            Ok(())
        } else {
            Err(EAGAIN)
        }
    }

    pub fn checkpoint(&self) -> Vec<u8> {
        let mut out = Encoder::new();
        out.word(1);
        out.word(self.mask);
        out.word(self.pending);
        out.word(self.alt_stack.address);
        out.word(self.alt_stack.size);
        out.word(u64::from(self.alt_stack.flags));
        out.word(self.saved_mask.is_some() as u64);
        out.word(self.saved_mask.unwrap_or(0));
        for action in self.actions.iter().skip(1) {
            out.word(action.handler);
            out.word(action.flags);
            out.word(action.restorer);
            out.word(action.mask);
        }
        out.finish()
    }

    pub fn restore(bytes: &[u8]) -> Result<Self, MigrationError> {
        let mut input = Decoder::new(bytes);
        if input.word()? != 1 {
            return Err(MigrationError::UnsupportedVersion);
        }
        let mut state = Self {
            mask: input.word()?,
            pending: input.word()?,
            alt_stack: AltStack {
                address: input.word()?,
                size: input.word()?,
                flags: u32::try_from(input.word()?).map_err(|_| MigrationError::InvalidData)?,
            },
            saved_mask: if input.flag()? {
                Some(input.word()?)
            } else {
                let value = input.word()?;
                if value != 0 {
                    return Err(MigrationError::InvalidData);
                }
                None
            },
            ..Default::default()
        };
        for action in state.actions.iter_mut().skip(1) {
            *action = SignalAction {
                handler: input.word()?,
                flags: input.word()?,
                restorer: input.word()?,
                mask: input.word()?,
            };
        }
        input.finish()?;
        state.mask &= !(bit(SIGKILL).unwrap() | bit(SIGSTOP).unwrap());
        Ok(state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_defer_signals_and_never_block_kill_or_stop() {
        let mut state = SignalState::default();
        state.update_mask(0, u64::MAX).unwrap();
        assert_eq!(state.mask() & bit(SIGKILL).unwrap(), 0);
        state.queue(2).unwrap();
        assert!(state.next().is_none());
        state.update_mask(1, bit(2).unwrap()).unwrap();
        assert_eq!(state.next().unwrap().0, 2);
    }

    #[test]
    fn action_reset_and_nodefer_follow_linux_flags() {
        let mut state = SignalState::default();
        state
            .set_action(
                10,
                SignalAction {
                    handler: 42,
                    flags: SA_RESETHAND | SA_NODEFER,
                    ..Default::default()
                },
            )
            .unwrap();
        state.queue(10).unwrap();
        let (_, action, old) = state.next().unwrap();
        assert_eq!(action.handler, 42);
        assert_eq!(old, 0);
        assert_eq!(state.mask(), 0);
        assert_eq!(state.action(10).unwrap(), SignalAction::default());
    }

    #[test]
    fn checkpoint_preserves_actions_masks_pending_and_alt_stack() {
        let mut state = SignalState::default();
        state
            .set_action(
                10,
                SignalAction {
                    handler: 42,
                    flags: SA_ONSTACK,
                    restorer: 99,
                    mask: 7,
                },
            )
            .unwrap();
        state.update_mask(0, 3).unwrap();
        state.queue(12).unwrap();
        state
            .set_alt_stack(AltStack {
                address: 0x1000,
                size: 8192,
                flags: 0,
            })
            .unwrap();
        assert_eq!(SignalState::restore(&state.checkpoint()).unwrap(), state);
    }
}
