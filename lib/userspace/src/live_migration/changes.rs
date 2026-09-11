//! Track exact record changes during live migration, including updates outside
//! control-plane handlers. Inactive services retain no duplicate snapshots.
use super::{Source, State};
use alloc::{collections::BTreeMap, vec::Vec};

#[derive(Default)]
pub struct RecordChanges {
    previous: BTreeMap<u64, Option<Vec<u8>>>,
}

impl RecordChanges {
    pub fn poll(&mut self, state: &impl State, source: &mut Source) {
        if !source.active() {
            self.previous.clear();
            return;
        }
        let keys = state.keys();
        let removed: Vec<_> = self
            .previous
            .keys()
            .copied()
            .filter(|key| !keys.contains(key))
            .collect();
        for key in removed {
            self.previous.remove(&key);
            source.changed(key);
        }
        for key in keys {
            if let Ok(record) = state.encode_record(key) {
                if self.previous.get(&key) != Some(&record) {
                    self.previous.insert(key, record);
                    source.changed(key);
                }
            }
        }
    }
}
