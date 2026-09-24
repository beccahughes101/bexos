use bexos_device_dataplane::{DataPlaneJournal, RetainedBuffer};

#[test]
fn ethernet_replay_preserves_handles_and_state() {
    let mut journal = DataPlaneJournal::ethernet(7, 100).unwrap();
    journal
        .register_buffer(RetainedBuffer {
            buffer_id: 4,
            vmo: 200,
            size_bytes: 4096,
        })
        .unwrap();
    journal.set_started(true);
    let encoded = journal.encode();
    let restored = DataPlaneJournal::decode(&encoded).unwrap();
    assert_eq!(restored, journal);
    assert_eq!(restored.replay().request_fifo_provider, 100);
    assert_eq!(restored.replay().buffers[0].vmo, 200);
}

#[test]
fn block_replay_preserves_both_fifo_ends() {
    let journal = DataPlaneJournal::block(8, 101, 102).unwrap();
    let replay = journal.replay();
    assert_eq!(replay.request_fifo_provider, 101);
    assert_eq!(replay.completion_fifo_provider, Some(102));
}
