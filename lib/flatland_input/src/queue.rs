use crate::{Event, Phase};
/// Overflow replaces the stream with an explicit reset. This never delivers a
/// release without a press or leaves a client believing a dropped key is held.
pub struct EventQueue<const N: usize> {
    items: [Option<Event>; N],
    head: usize,
    len: usize,
    pub overflows: u64,
}
impl<const N: usize> Default for EventQueue<N> {
    fn default() -> Self {
        Self {
            items: [None; N],
            head: 0,
            len: 0,
            overflows: 0,
        }
    }
}
impl<const N: usize> EventQueue<N> {
    pub fn push(&mut self, event: Event) {
        if N == 0 {
            return;
        }
        if self.len > 0 {
            let last = (self.head + self.len - 1) % N;
            if let (Some(Event::Pointer(a)), Event::Pointer(b)) = (self.items[last], event) {
                if a.device == b.device
                    && a.id == b.id
                    && a.buttons == b.buttons
                    && a.phase == b.phase
                    && matches!(b.phase, Phase::Move | Phase::Hover)
                    && a.scroll_x == 0.
                    && a.scroll_y == 0.
                    && b.scroll_x == 0.
                    && b.scroll_y == 0.
                {
                    self.items[last] = Some(event);
                    return;
                }
            }
        }
        if self.len == N {
            self.items.fill(None);
            self.head = 0;
            self.len = 1;
            self.items[0] = Some(Event::Reset);
            self.overflows += 1;
            return;
        }
        self.items[(self.head + self.len) % N] = Some(event);
        self.len += 1;
    }
    pub fn pop(&mut self) -> Option<Event> {
        if self.len == 0 {
            return None;
        }
        let value = self.items[self.head].take();
        self.head = (self.head + 1) % N;
        self.len -= 1;
        value
    }
    pub fn iter(&self) -> impl Iterator<Item = Event> + '_ {
        (0..self.len).filter_map(|i| self.items[(self.head + i) % N])
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}
