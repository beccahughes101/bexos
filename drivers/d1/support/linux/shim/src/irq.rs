#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IrqVector(pub u32);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Event {
    signaled: bool,
}

impl Event {
    pub fn signal(&mut self) {
        self.signaled = true;
    }

    pub fn clear(&mut self) {
        self.signaled = false;
    }

    pub const fn is_signaled(self) -> bool {
        self.signaled
    }
}
