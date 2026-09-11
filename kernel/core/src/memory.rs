pub const PAGE_SIZE: usize = 4096;
pub const DEFAULT_RECYCLED_FRAMES: usize = 128;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Frame {
    pub start: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PageFrameAllocator {
    start: usize,
    next: usize,
    end: usize,
    recycled: [Option<Frame>; DEFAULT_RECYCLED_FRAMES],
}

impl PageFrameAllocator {
    pub const fn empty() -> Self {
        Self {
            start: 0,
            next: 0,
            end: 0,
            recycled: [None; DEFAULT_RECYCLED_FRAMES],
        }
    }

    pub fn new(start: usize, end: usize) -> Self {
        let next = align_up(start, PAGE_SIZE);
        let end = align_down(end, PAGE_SIZE);
        if next > end {
            return Self {
                start: end,
                next: end,
                end,
                recycled: [None; DEFAULT_RECYCLED_FRAMES],
            };
        }
        Self {
            start: next,
            next,
            end,
            recycled: [None; DEFAULT_RECYCLED_FRAMES],
        }
    }

    pub fn allocate(&mut self) -> Option<Frame> {
        if let Some(slot) = self.recycled.iter_mut().find(|slot| slot.is_some()) {
            return slot.take();
        }
        if self.next >= self.end {
            return None;
        }

        let frame = Frame { start: self.next };
        self.next += PAGE_SIZE;
        Some(frame)
    }

    pub fn free(&mut self, frame: Frame) -> bool {
        if frame.start % PAGE_SIZE != 0 || frame.start < self.start || frame.start >= self.end {
            return false;
        }
        if self
            .recycled
            .iter()
            .flatten()
            .any(|recycled| recycled.start == frame.start)
        {
            return false;
        }
        let Some(slot) = self.recycled.iter_mut().find(|slot| slot.is_none()) else {
            return false;
        };
        *slot = Some(frame);
        true
    }

    pub fn remaining_frames(&self) -> usize {
        self.end.saturating_sub(self.next) / PAGE_SIZE + self.recycled_len()
    }

    pub fn recycled_len(&self) -> usize {
        self.recycled.iter().flatten().count()
    }
}

pub const fn align_down(value: usize, alignment: usize) -> usize {
    value & !(alignment - 1)
}

pub const fn align_up(value: usize, alignment: usize) -> usize {
    align_down(value + alignment - 1, alignment)
}
