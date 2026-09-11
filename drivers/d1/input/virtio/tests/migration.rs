use bexos_graphics_runtime::migration::Runtime;
use bexos_userspace::{Channel, live_migration::State};
use bexos_virtio_input::state::Input;
#[test]
fn stream_capability_sequence_and_overflow_survive_replacement() {
    let old = Runtime::new(
        Channel(1),
        Some(Channel(2)),
        Input {
            sink: Some(Channel(3)),
            sequence: 73,
            reset: true,
            reports: 500,
            ..Default::default()
        },
    );
    let mut new = Runtime::<Input>::empty();
    for k in old.keys() {
        new.adopt_record(k, old.encode_record(k).unwrap().as_deref())
            .unwrap();
    }
    new.finish_adoption().unwrap();
    assert_eq!(new.component.sink.unwrap().0, 3);
    assert_eq!(new.component.sequence, 73);
    assert!(new.component.reset);
    assert_eq!(new.component.reports, 500);
}

#[test]
fn full_device_report_fits_the_bounded_stream_receiver() {
    use input_fidl::{FidlDecode, FidlEncode, InputReport, RawInputEvent, WireVector};
    let events = [RawInputEvent {
        kind: 3,
        code: 0x35,
        value: 32767,
    }; 64];
    let report = InputReport {
        sequence: 9,
        reset: false,
        events: WireVector::from_slice(&events),
    };
    let mut bytes = [0; 3584];
    let encoded = report.encode(&mut bytes, &mut []).unwrap();
    assert_eq!(encoded.handles, 0);
    let restored = InputReport::decode(&bytes[..encoded.bytes], &[]).unwrap();
    assert_eq!(restored.events.len(), 64);
    assert_eq!(restored.events.get(63).unwrap().value, 32767);
    assert!(InputReport::decode(&bytes[..8], &[]).is_err());
}

#[test]
fn device_used_batches_wrap_and_reject_corruption_before_consumption() {
    use bexos_flatland_input::virtio::RawEvent;
    use bexos_virtio_input::report_queue::{self, Error};
    #[repr(align(8))]
    struct Queue([u8; 12288]);
    let mut queue = Queue([0; 12288]);
    let mut reports = [0; 512];
    let mut out = [RawEvent::default(); 64];
    let used = u16::MAX - 1;
    queue.0[8194..8196].copy_from_slice(&1u16.to_le_bytes());
    for (index, id) in [9u32, 3, 5].into_iter().enumerate() {
        let offset = 8196 + (used.wrapping_add(index as u16) as usize % 64) * 8;
        queue.0[offset..offset + 4].copy_from_slice(&id.to_le_bytes());
        queue.0[offset + 4..offset + 8].copy_from_slice(&8u32.to_le_bytes());
        reports[id as usize * 8..id as usize * 8 + 2].copy_from_slice(&1u16.to_le_bytes());
    }
    let batch = report_queue::read(&queue.0, &reports, used, &mut out).unwrap();
    assert_eq!(batch.count, 3);
    assert_eq!(&batch.ids[..3], &[9, 3, 5]);
    assert_eq!(out[2].kind, 1);
    let before = queue.0;
    // Reusing a descriptor twice in one used batch is a device error.
    queue.0[8196..8200].copy_from_slice(&9u32.to_le_bytes());
    assert!(matches!(
        report_queue::read(&queue.0, &reports, used, &mut out),
        Err(Error::Descriptor)
    ));
    assert_eq!(&queue.0[4096..8192], &before[4096..8192]);
    queue.0[8194..8196].copy_from_slice(&used.wrapping_add(65).to_le_bytes());
    assert!(matches!(
        report_queue::read(&queue.0, &reports, used, &mut out),
        Err(Error::Count)
    ));
}
