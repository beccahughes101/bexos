//! Reclaim dead storage without changing any live object or endpoint identity.
use super::*;
impl<B: Backend> Runtime<B> {
    pub(super) fn insert_channel(&mut self) -> usize {
        let identity = self
            .channels
            .iter()
            .map(|c| c.identity)
            .max()
            .map_or(1, |value| {
                value.checked_add(2).expect("channel identity exhausted")
            });
        let free = self.channels.iter().enumerate().find_map(|(id, c)| {
            (c.refs == [0, 0]
                && c.queues.iter().all(VecDeque::is_empty)
                && c.calls.iter().all(VecDeque::is_empty)
                && !self.handles.iter().flatten().any(
                    |h| matches!(h.object, Object::ReplyToken(channel, _, _) if channel == id),
                ))
            .then_some(id)
        });
        let channel = Channel {
            identity,
            queues: [VecDeque::new(), VecDeque::new()],
            calls: [VecDeque::new(), VecDeque::new()],
            refs: [0, 0],
            next_call_id: 1,
            policy: ChannelPolicy::disabled(),
        };
        let id = free.unwrap_or(self.channels.len());
        if id == self.channels.len() {
            self.channels.push(channel);
        } else {
            self.channels[id] = channel;
        }
        self.changed(CHANNEL, id);
        id
    }
    pub(super) fn insert_vmo(&mut self, vmo: Vmo) -> usize {
        // Deferred retirement keeps Some(Vmo) until reclamation is complete.
        let id = self
            .vmos
            .iter()
            .position(Option::is_none)
            .unwrap_or(self.vmos.len());
        if id == self.vmos.len() {
            self.vmos.push(Some(vmo));
        } else {
            self.vmos[id] = Some(vmo);
        }
        self.changed(VMO, id);
        id
    }
}
