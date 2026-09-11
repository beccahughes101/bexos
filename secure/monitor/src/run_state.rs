//! Software state for intercepted HLT. A host scheduling tick is not a guest
//! wakeup. Only a deliverable guest interrupt (or explicit vCPU reset) resumes
//! instructions after HLT.
#[derive(Clone, Copy, Default)]
pub struct RunState {
    halted: bool,
}
impl RunState {
    pub(crate) fn halted(self) -> bool {
        self.halted
    }
    pub(crate) fn from_halted(halted: bool) -> Self {
        Self { halted }
    }
    pub fn halt(&mut self) {
        self.halted = true;
    }
    pub fn reset(&mut self) {
        self.halted = false;
    }
    pub fn ready(&mut self, interrupt: bool, enabled: bool) -> bool {
        if self.halted && interrupt && enabled {
            self.halted = false;
        }
        !self.halted
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scheduling_and_masked_interrupts_cannot_resume_halted_code() {
        let mut state = RunState::default();
        assert!(state.ready(false, false));
        state.halt();
        for _ in 0..100 {
            assert!(!state.ready(false, true));
            assert!(!state.ready(true, false));
        }
        assert!(state.ready(true, true));
        assert!(state.ready(false, false));
        state.halt();
        state.reset();
        assert!(state.ready(false, false));
    }
}
