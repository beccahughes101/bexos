#[test]
fn diskimage_guest_entry_is_tokio_future() {
    let _future = bexos_d1_diskimage::guest::main(0);
}

#[test]
fn encrypted_image_errors_have_no_invalid_handle_to_transfer() {
    use diskimage_fidl::*;

    let mut bytes = [0; 192];
    let mut handles = [HandleRef { raw: 0 }; 1];
    let mut response = DiskImageManagerCreateEncryptedResponse {
        status: Status::ErrAccessDenied,
        block_device: None,
        image_uuid: [0; 16],
        virtual_size_bytes: 0,
    };
    let encoded = response.encode(&mut bytes, &mut handles).unwrap();
    assert_eq!(encoded.handles, 0);
    let decoded =
        DiskImageManagerCreateEncryptedResponse::decode(&bytes[..encoded.bytes], &[]).unwrap();
    assert_eq!(decoded.status, Status::ErrAccessDenied);
    assert!(decoded.block_device.is_none());

    response.status = Status::Ok;
    response.block_device = Some(BlockDeviceBinding {
        endpoint: HandleRef { raw: 42 },
    });
    let encoded = response.encode(&mut bytes, &mut handles).unwrap();
    assert_eq!(encoded.handles, 1);
    let decoded = DiskImageManagerCreateEncryptedResponse::decode(
        &bytes[..encoded.bytes],
        &handles[..encoded.handles],
    )
    .unwrap();
    assert_eq!(decoded.block_device.unwrap().endpoint.raw, 42);
    assert!(
        DiskImageManagerCreateEncryptedResponse::decode(&bytes[..encoded.bytes], &[],).is_err()
    );
}
