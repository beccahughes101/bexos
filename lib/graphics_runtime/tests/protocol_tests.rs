use graphics_fidl::*;
#[test]
fn owned_response_rejects_extra_unclaimed_handles() {
    use bexos_graphics_runtime::response::decode_owned;
    let mut bytes = [0; 128];
    let mut hs = [HandleRef { raw: 0 }; 4];
    let q = FlatlandSessionPresentResponse { status: Status::Ok };
    let n = q.encode(&mut bytes, &mut hs).unwrap();
    let decode = |handles: &[HandleRef]| {
        decode_owned::<FlatlandSessionPresentResponse>(
            &bytes[..n.bytes],
            handles,
            &mut [0; 128],
            &mut [HandleRef { raw: 0 }; 4],
        )
    };
    assert!(decode(&[]).is_ok());
    assert!(decode(&[HandleRef { raw: 99 }]).is_err());
    let q = FlatlandSessionGetViewReferenceResponse {
        status: Status::Ok,
        view: Some(ViewReference {
            token: HandleRef { raw: 42 },
            view_id: 19,
        }),
    };
    let n = q.encode(&mut bytes, &mut hs).unwrap();
    let decode = |handles: &[HandleRef]| {
        decode_owned::<FlatlandSessionGetViewReferenceResponse>(
            &bytes[..n.bytes],
            handles,
            &mut [0; 128],
            &mut [HandleRef { raw: 0 }; 4],
        )
    };
    assert_eq!(decode(&hs[..1]).unwrap().view.unwrap().token.raw, 42);
    assert!(decode(&[]).is_err());
    assert!(decode(&[HandleRef { raw: 42 }, HandleRef { raw: 99 }]).is_err());
}
#[test]
fn presentation_completion_requires_a_complete_successful_fence() {
    use bexos_graphics_runtime::presentation::decode_completion;
    let mut bytes = [0; 16];
    let response = DisplayCoordinatorPresentResponse {
        status: Status::Ok,
        fence: 91,
    };
    let n = response.encode(&mut bytes, &mut []).unwrap();
    assert_eq!(n.bytes, 16);
    assert_eq!(decode_completion(&bytes), Ok(91));
    for len in 0..16 {
        assert!(decode_completion(&bytes[..len]).is_err());
    }
    let mut trailing = bytes.to_vec();
    trailing.push(0);
    assert!(decode_completion(&trailing).is_err());
    DisplayCoordinatorPresentResponse {
        status: Status::Ok,
        fence: 0,
    }
    .encode(&mut bytes, &mut [])
    .unwrap();
    assert!(decode_completion(&bytes).is_err());
    DisplayCoordinatorPresentResponse {
        status: Status::ErrIo,
        fence: 91,
    }
    .encode(&mut bytes, &mut [])
    .unwrap();
    assert_eq!(decode_completion(&bytes), Err(Status::ErrIo));
}
#[test]
fn rejected_handoff_has_no_invalid_handle_to_transfer() {
    let mut bytes = [0; 128];
    let mut handles = [HandleRef { raw: 0 }; 1];
    let response = ProgressTrackerHandoverToCompositorResponse {
        status: Status::ErrInvalidArgs,
        frame: None,
    };
    let n = response.encode(&mut bytes, &mut handles).unwrap();
    assert_eq!(n.handles, 0);
    let decoded =
        ProgressTrackerHandoverToCompositorResponse::decode(&bytes[..n.bytes], &[]).unwrap();
    assert_eq!(decoded.status, Status::ErrInvalidArgs);
    assert!(decoded.frame.is_none());
}
#[test]
fn snapshot_requires_its_advertised_vmo() {
    let mut bytes = [0; 128];
    let mut handles = [HandleRef { raw: 0 }; 1];
    let response = DisplayCoordinatorSnapshotResponse {
        status: Status::Ok,
        frame: Some(DisplayFrame {
            buffer: HandleRef { raw: 7 },
            surface: Surface {
                width: 8,
                height: 8,
                stride: 32,
                format: 1,
            },
            generation: 3,
        }),
    };
    let n = response.encode(&mut bytes, &mut handles).unwrap();
    assert_eq!(n.handles, 1);
    assert!(DisplayCoordinatorSnapshotResponse::decode(&bytes[..n.bytes], &[]).is_err());
}

#[test]
fn event_loop_ipc_fits_fixed_buffers_at_admission_limits() {
    use kernel_fidl::{FidlEncode, HandleRef, Signals, WireVector};
    let items = [kernel_fidl::InlineVectorStruct1 {
        h: HandleRef { raw: 1 },
        signals: Signals(3),
    }; 64];
    let wait = kernel_fidl::TaskControlWaitManyRequest {
        items: WireVector::from_slice(&items),
        deadline_nanos: 1_000_000,
    };
    let mut handles = [HandleRef { raw: 0 }; 64];
    let encoded = wait.encode(&mut [0; 2048], &mut handles).unwrap();
    assert_eq!(encoded.handles, 64);
    let write = kernel_fidl::ChannelControlWriteMessageRequest {
        channel: HandleRef { raw: 1 },
        data: &[0; 2048],
        handles: &[],
    };
    assert_eq!(
        write.encode(&mut [0; 4096], &mut handles).unwrap().handles,
        1
    );
    let response = kernel_fidl::TaskControlWaitManyResponse {
        status: kernel_fidl::Status::Ok,
        satisfied_index: 63,
        observed_signals: Signals(3),
    };
    response.encode(&mut [0; 32], &mut []).unwrap();
}

#[test]
fn full_input_batch_fits_receive_storage_with_kernel_envelope() {
    let events = [NormalizedInputEvent {
        kind: 2,
        device: 1,
        id: 1,
        phase: 1,
        x: 0.0,
        y: 0.0,
        buttons: 0,
        scroll_x: 0.0,
        scroll_y: 0.0,
        code: 30,
        key_state: 1,
        modifiers: 0,
        unicode: 97,
    }; 8];
    let mut bytes = [0; bexos_graphics_runtime::flatland::INPUT_RESPONSE_BYTES
        - bexos_graphics_runtime::stream::ENVELOPE_BYTES];
    let n = FlatlandSessionReadInputResponse {
        status: Status::Ok,
        events: WireVector::from_slice(&events),
    }
    .encode(&mut bytes, &mut [])
    .unwrap();
    let decoded = FlatlandSessionReadInputResponse::decode(&bytes[..n.bytes], &[]).unwrap();
    assert_eq!(decoded.events.len(), 8);
    assert_eq!(decoded.events.get(7).unwrap().unicode, 97);
}
