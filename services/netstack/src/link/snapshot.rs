use super::*;
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};

pub const BACKLOG_CHUNKS: usize = 8;
const CHUNK_FRAMES: usize = SLOT_COUNT / BACKLOG_CHUNKS;

impl PacketLink {
    /// A versioned handoff with a link must include its queue ownership record.
    pub fn expect_queues(&mut self) {
        self.queues_restored = false;
    }

    pub fn checkpoint_queues(&self) -> Result<Vec<u8>, Error> {
        let mut w = Encoder::new();
        w.word(1);
        w.word(self.model.next_req_id);
        w.word(self.model.next_tx_slot as u64);
        w.word(self.model.next_rx_slot as u64);
        encode_entries(&mut w, self.model.supplied_rx.iter())?;
        encode_entries(&mut w, self.model.completed_rx.iter())?;
        encode_entries(&mut w, self.model.in_flight_tx.iter())?;
        encode_entries(&mut w, self.model.completed_tx.iter())?;
        w.word(self.rx_backlog.len() as u64);
        Ok(w.finish())
    }

    pub fn adopt_queues(&mut self, bytes: &[u8]) -> Result<(), Error> {
        let mut r = Decoder::new(bytes);
        if r.word()? != 1 {
            return Err(Error::UnsupportedVersion);
        }
        let next_req_id = r.word()?;
        let next_tx_slot = r.count(SLOT_COUNT - 1)?;
        let next_rx_slot = r.count(SLOT_COUNT - 1)?;
        let supplied_rx = decode_entries(&mut r, FrameOpcode::RxSupply)?;
        let completed_rx = decode_entries(&mut r, FrameOpcode::RxComplete)?;
        let in_flight_tx = decode_entries(&mut r, FrameOpcode::TxSend)?;
        let completed_tx = decode_entries(&mut r, FrameOpcode::TxComplete)?;
        if next_req_id == 0 || supplied_rx.len() + completed_rx.len() > SLOT_COUNT {
            return Err(Error::InvalidData);
        }
        let backlog_len = r.count(SLOT_COUNT)?;
        r.finish()?;
        self.model = LinkModel {
            next_req_id,
            next_tx_slot,
            next_rx_slot,
            supplied_rx,
            completed_rx: completed_rx.into(),
            in_flight_tx,
            completed_tx,
        };
        self.rx_backlog.resize_with(backlog_len, Vec::new);
        self.queues_restored = true;
        Ok(())
    }

    pub fn checkpoint_backlog(&self, chunk: usize) -> Result<Vec<u8>, Error> {
        if chunk >= BACKLOG_CHUNKS {
            return Err(Error::InvalidData);
        }
        let mut w = Encoder::new();
        let frames: Vec<_> = self
            .rx_backlog
            .iter()
            .skip(chunk * CHUNK_FRAMES)
            .take(CHUNK_FRAMES)
            .collect();
        w.word(frames.len() as u64);
        for frame in frames {
            w.bytes(frame);
        }
        Ok(w.finish())
    }

    pub fn adopt_backlog(&mut self, chunk: usize, bytes: &[u8]) -> Result<(), Error> {
        if chunk >= BACKLOG_CHUNKS {
            return Err(Error::InvalidData);
        }
        let mut r = Decoder::new(bytes);
        let count = r.count(CHUNK_FRAMES)?;
        let expected = self
            .rx_backlog
            .len()
            .saturating_sub(chunk * CHUNK_FRAMES)
            .min(CHUNK_FRAMES);
        if count != expected {
            return Err(Error::InvalidData);
        }
        for index in 0..count {
            let frame = r.bytes(SLOT_SIZE)?;
            if frame.is_empty() {
                return Err(Error::InvalidData);
            }
            self.rx_backlog[chunk * CHUNK_FRAMES + index] = frame.to_vec();
        }
        r.finish()
    }

    pub fn queues_valid(&self) -> bool {
        let rx = self
            .model
            .supplied_rx
            .iter()
            .chain(self.model.completed_rx.iter())
            .collect::<Vec<_>>();
        let tx = self.model.in_flight_tx.iter().collect::<Vec<_>>();
        self.queues_restored
            && [
                (&rx, self.resources.rx_vmo_id),
                (&tx, self.resources.tx_vmo_id),
            ]
            .iter()
            .all(|(entries, vmo)| {
                entries.iter().enumerate().all(|(index, entry)| {
                    entry.vmo_id == *vmo
                        && entry.offset as usize % SLOT_SIZE == 0
                        && (entry.offset as usize) < SLOT_COUNT * SLOT_SIZE
                        && entry.length as usize <= SLOT_SIZE
                        && !entries[..index].iter().any(|previous| {
                            previous.offset == entry.offset || previous.req_id == entry.req_id
                        })
                })
            })
            && self
                .rx_backlog
                .iter()
                .all(|frame| !frame.is_empty() && frame.len() <= SLOT_SIZE)
    }
}

fn encode_entries<'a>(
    w: &mut Encoder,
    entries: impl Iterator<Item = &'a FrameEntry>,
) -> Result<(), Error> {
    let entries: Vec<_> = entries.collect();
    w.word(entries.len() as u64);
    for entry in entries {
        let mut bytes = [0; 64];
        let encoded = entry
            .encode(&mut bytes, &mut [])
            .map_err(|_| Error::InvalidData)?;
        w.bytes(&bytes[..encoded.bytes]);
    }
    Ok(())
}

fn decode_entries(r: &mut Decoder<'_>, opcode: FrameOpcode) -> Result<Vec<FrameEntry>, Error> {
    let mut entries = Vec::new();
    for _ in 0..r.count(SLOT_COUNT)? {
        let entry = FrameEntry::decode(r.bytes(64)?, &[]).map_err(|_| Error::InvalidData)?;
        if entry.opcode != opcode || entry.req_id == 0 {
            return Err(Error::InvalidData);
        }
        entries.push(entry);
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retained_queues_and_backlog_round_trip_without_reusing_owned_slots() {
        let resources = LinkResources {
            control: 1,
            fifo: 2,
            rx_vmo: 3,
            tx_vmo: 4,
            rx_vaddr: 0x1000,
            tx_vaddr: 0x2000,
            rx_vmo_id: 3,
            tx_vmo_id: 4,
            mtu: 1500,
            mac: [0; 6],
        };
        let mut source = PacketLink::from_resources(resources);
        let supplied = source.model.rx_supply_entries(3, SLOT_COUNT);
        source.model.complete(FrameEntry {
            opcode: FrameOpcode::RxComplete,
            length: 64,
            ..supplied[7]
        });
        let tx = source.model.tx_entry(4, 96).unwrap();
        source.push_rx_backlog(b"first");
        source.push_rx_backlog(b"second");
        let mut target = PacketLink::from_resources(resources);
        target.expect_queues();
        assert!(!target.queues_valid());
        target
            .adopt_queues(&source.checkpoint_queues().unwrap())
            .unwrap();
        assert!(!target.queues_valid(), "backlog payload is required");
        for chunk in 0..BACKLOG_CHUNKS {
            target
                .adopt_backlog(chunk, &source.checkpoint_backlog(chunk).unwrap())
                .unwrap();
        }
        assert!(target.queues_valid());
        assert_eq!(source.model, target.model);
        assert_eq!(source.rx_backlog, target.rx_backlog);
        assert!(target.model.rx_supply_entries(3, SLOT_COUNT).is_empty());
        assert_ne!(target.model.tx_entry(4, 96).unwrap().offset, tx.offset);
        assert_eq!(target.model.pop_rx().unwrap().req_id, supplied[7].req_id);
        assert_eq!(
            target.model.rx_supply_entries(3, SLOT_COUNT)[0].offset,
            supplied[7].offset
        );
    }
}
