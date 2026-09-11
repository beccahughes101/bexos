use bexos_flatland_input::{
    Event, Key,
    virtio::{Device, RawEvent},
};
use bexos_migration::codec::{Decoder, Encoder};
use bexos_scened::input_config::decode;
#[test]
fn packaged_policy_and_custom_mapping_control_real_normalization_and_migrate() {
    let mut config = include_bytes!(env!("INPUT_CONFIG")).to_vec();
    // Key 30 -> z/Z. Append an explicitly encoded bounded protobuf override.
    config.extend_from_slice(&[34, 6, 8, 30, 16, 122, 24, 90]);
    let policy = decode(&config).unwrap();
    assert_eq!(policy.keymap.translate(30, false, false), 122);
    assert_eq!(policy.keymap.translate(30, true, false), 90);
    let mut policy = policy;
    policy.repeat_delay = 200_000;
    policy.repeat_interval = 10_000;
    let mut device = Device::new(1);
    policy.apply(&mut device).unwrap();
    assert!(matches!(
        device.feed_at(
            RawEvent {
                kind: 1,
                code: 30,
                value: 1
            },
            800.,
            600.,
            10
        ),
        Some(Event::Key(Key {
            unicode: 122,
            state: 1,
            ..
        }))
    ));
    assert!(device.repeat(200_009, 800., 600.).is_none());
    let mut w = Encoder::new();
    device.encode(&mut w);
    let bytes = w.finish();
    let mut r = Decoder::new(&bytes);
    let mut device = Device::decode(&mut r).unwrap();
    r.finish().unwrap();
    assert!(matches!(
        device.repeat(200_010, 800., 600.),
        Some(Event::Key(Key {
            unicode: 122,
            state: 2,
            ..
        }))
    ));
    assert_eq!(device.repeat_at, 210_010);
    let mut w = Encoder::new();
    policy.encode(&mut w);
    let bytes = w.finish();
    let mut r = Decoder::new(&bytes);
    let restored = bexos_flatland_input::settings::Settings::decode(&mut r).unwrap();
    r.finish().unwrap();
    assert_eq!(restored.keymap.normal, policy.keymap.normal);
}
#[test]
fn malformed_duplicate_and_out_of_range_policy_is_rejected() {
    let bytes = include_bytes!(env!("INPUT_CONFIG"));
    for extra in [
        &[8, 1][..],
        &[34, 6, 8, 128, 1, 16, 1][..],
        &[34, 6, 8, 1, 8, 1, 8, 1][..],
        &[255][..],
    ] {
        let mut invalid = bytes.to_vec();
        invalid.extend_from_slice(extra);
        assert!(decode(&invalid).is_err());
    }
    assert!(decode(&[]).is_err());
    let mut duplicate = bytes.to_vec();
    duplicate.extend_from_slice(&[34, 6, 8, 30, 16, 122, 24, 90, 34, 6, 8, 30, 16, 122, 24, 90]);
    assert!(decode(&duplicate).is_err());
}
