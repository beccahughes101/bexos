use bexos_usb_bot::Runtime;
use bexos_userspace::{Channel, live_migration::State};

#[test]
fn block_buffers_fifo_and_write_watermark_survive_replacement() {
    let mut runtime = Runtime::new(Channel(1), Some(Channel(2)));
    runtime.interface = Some(Channel(3));
    runtime.fifos.push(Channel(4));
    runtime.transport.block_count = 128;
    runtime.transport.completed_write_watermark = 77;
    runtime.block.buffers.insert(
        1,
        bexos_usb_bot::block::Buffer {
            handle: 8,
            mapped: 0x9000,
            size_bytes: 4096,
        },
    );
    let mut new = Runtime::empty();
    for key in runtime.keys() {
        new.adopt_record(key, runtime.encode_record(key).unwrap().as_deref())
            .unwrap();
    }
    new.finish_adoption().unwrap();
    assert_eq!(new.interface.unwrap().0, 3);
    assert_eq!(new.fifos[0].0, 4);
    assert_eq!(new.transport.completed_write_watermark, 77);
    assert_eq!(new.block.buffers[&1].mapped, 0x9000);
}
