use bexos_migration::codec::{Decoder, Encoder};
use bexos_virtio_gpu_protocol::*;
fn put(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}
#[test]
fn control_acknowledgements_cannot_be_used_as_gpu_fences() {
    let mut response = [0; 24];
    put(&mut response, 0, 0x1100);
    control_acknowledgement(&response, 0x1100).unwrap();
    assert!(completion(&response, 0x1100, 0).is_err());
    assert!(control_acknowledgement(&response, 0x1101).is_err());
    for i in 4..24 {
        response[i] = 1;
        assert!(control_acknowledgement(&response, 0x1100).is_err());
        response[i] = 0;
    }
}

#[test]
fn completion_rejects_wrong_fence_context_and_unfenced_responses() {
    let mut response = [0; 24];
    put(&mut response, 0, 0x1100);
    put(&mut response, 4, 1);
    response[8..16].copy_from_slice(&42u64.to_le_bytes());
    completion(&response, 0x1100, 42).unwrap();
    assert!(completion(&response, 0x1100, 43).is_err());
    put(&mut response, 16, 1);
    assert!(completion(&response, 0x1100, 42).is_err());
    completion_for_context(&response, 0x1100, 42, 1).unwrap();
    put(&mut response, 16, 0);
    put(&mut response, 4, 0);
    assert!(completion(&response, 0x1100, 42).is_err());
}

#[test]
fn venus_resource_ownership_and_limits_survive_migration() {
    use transport::*;
    let mut registry = Registry::default();
    let a = registry.create_context(70).unwrap();
    let b = registry.create_context(80).unwrap();
    assert!(registry.create_resource(80, a, 4096).is_err());
    assert!(registry.create_resource(70, a, 4095).is_err());
    let resource = registry.create_resource(70, a, 4096).unwrap();
    assert!(registry.remove_context(70, a).is_err());
    assert!(registry.remove_resource(80, b, resource).is_err());
    assert!(registry.remove_resource(80, a, resource).is_err());
    let mut encoded = Encoder::new();
    registry.encode(&mut encoded);
    let bytes = encoded.finish();
    let mut decoder = Decoder::new(&bytes);
    let mut restored = Registry::decode(&mut decoder).unwrap();
    decoder.finish().unwrap();
    assert_eq!(registry, restored);
    for len in 0..bytes.len() {
        assert!(Registry::decode(&mut Decoder::new(&bytes[..len])).is_err());
    }
    restored.remove_resource(70, a, resource).unwrap();
    assert!(restored.create_resource(70, a, 4096).unwrap() > resource);
    for _ in 0..MAX_CONTEXTS - 2 {
        restored.create_context(70).unwrap();
    }
    assert_eq!(restored.create_context(70), Err(Error::Capacity));
    let mut limited = Registry::default();
    let c = limited.create_context(1).unwrap();
    for _ in 0..4 {
        limited.create_resource(1, c, MAX_RESOURCE_BYTES).unwrap();
    }
    assert_eq!(limited.create_resource(1, c, 4096), Err(Error::Capacity));
}

#[test]
fn venus_negotiation_requires_all_transport_features() {
    use transport::*;
    for bits in [
        0,
        FEATURE_VIRGL,
        FEATURE_VIRGL | FEATURE_BLOB,
        FEATURE_CONTEXT_INIT | FEATURE_BLOB,
    ] {
        assert_eq!(negotiated_features((1 << 32) | bits), 1 << 32);
    }
    assert_eq!(
        negotiated_features((1 << 32) | VENUS_FEATURES),
        (1 << 32) | VENUS_FEATURES
    );
    assert_eq!(
        negotiated_features((1 << 32) | (1 << 33) | VENUS_FEATURES),
        (1 << 32) | (1 << 33) | VENUS_FEATURES
    );
}
#[test]
fn discovery_validates_host_bounds_and_preserves_capabilities() {
    let mut c = Capabilities {
        offered: (1 << 32) | FEATURE_VIRGL | FEATURE_BLOB | FEATURE_CONTEXT_INIT,
        negotiated: 1 << 32,
        ..Default::default()
    };
    let mut mode = [0; 408];
    put(&mut mode, 0, 0x1101);
    put(&mut mode, 32, 1920);
    put(&mut mode, 36, 1080);
    put(&mut mode, 40, 1);
    c.parse_modes(&mode, 1).unwrap();
    assert_eq!(c.modes[0].width, 1920);
    let before = c.clone();
    put(&mut mode, 32, 16385);
    assert!(c.parse_modes(&mode, 1).is_err());
    assert_eq!(c, before);
    let mut cap = [0; 40];
    put(&mut cap, 0, 0x1102);
    put(&mut cap, 24, 4);
    put(&mut cap, 32, 160);
    c.add_capset(&cap).unwrap();
    assert!(c.venus_offered());
    assert_eq!(c.negotiated, 1 << 32);
    assert!(c.add_capset(&cap).is_err());
    let mut w = Encoder::new();
    c.encode(&mut w);
    let bytes = w.finish();
    let mut r = Decoder::new(&bytes);
    assert_eq!(Capabilities::decode(&mut r).unwrap(), c);
    r.finish().unwrap();
    for len in 0..bytes.len() {
        assert!(Capabilities::decode(&mut Decoder::new(&bytes[..len])).is_err());
    }
}

#[test]
fn host_aperture_placement_checks_holes_overlap_and_overflow() {
    use aperture::Aperture;
    let aperture = Aperture::new(0x2000_0000, 0x10000, 0x1000, 0xf000).unwrap();
    let occupied = [(0x4000, 0x4000), (0, 0x2000)];
    assert_eq!(aperture.allocate(0x2000, &occupied).unwrap(), 0x2000);
    assert_eq!(aperture.allocate(0x3000, &occupied).unwrap(), 0x8000);
    assert_eq!(aperture.allocate(0x8000, &occupied), Err(Error::Capacity));
    assert!(
        aperture
            .allocate(0x1000, &[(0, 0x2000), (0x1000, 0x1000)])
            .is_err()
    );
    assert!(Aperture::new(u64::MAX - 4095, 8192, 0, 8192).is_err());
    assert!(Aperture::new(0x1000, 4096, 4096, 4096).is_err());
    assert!(aperture.allocate(u64::MAX, &[]).is_err());
}
