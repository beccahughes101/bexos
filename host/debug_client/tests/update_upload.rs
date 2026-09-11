use bexos_debug_client::{DebugClient, DebugTransport};
use bexos_debug_wire::*;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Received {
    streams: [Vec<u8>; 2],
    chunks: usize,
    committed: bool,
}

struct Peer {
    received: Arc<Mutex<Received>>,
    response: Vec<u8>,
    reject_chunk: Option<usize>,
}
impl DebugTransport for Peer {
    fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        assert!(self.response.is_empty(), "wait for each acknowledgement");
        let (frame, consumed) = parse_frame(bytes).unwrap();
        assert_eq!(consumed, bytes.len());
        assert!(frame.payload.len() <= MAX_PAYLOAD_LEN);
        let mut received = self.received.lock().unwrap();
        let mut status = 0;
        match frame.method_id {
            METHOD_BEGIN_UPDATE_UPLOAD => {
                let request = decode_update_upload_begin(&frame.payload).unwrap();
                assert_eq!(request.upload_id, u64::MAX);
            }
            METHOD_WRITE_UPDATE_CHUNK => {
                let request = decode_update_chunk(&frame.payload).unwrap();
                assert_eq!(request.upload_id, u64::MAX);
                received.chunks += 1;
                if self.reject_chunk == Some(received.chunks) {
                    status = -8;
                } else {
                    let stream = &mut received.streams[request.stream as usize - 1];
                    assert_eq!(request.offset, stream.len() as u64);
                    stream.extend_from_slice(&request.bytes);
                }
            }
            METHOD_COMMIT_UPDATE_UPLOAD => received.committed = true,
            method => panic!("unexpected upload method {method}"),
        }
        let mut payload = Vec::new();
        encode_debug_status(
            &DebugStatusResponse {
                status,
                message: String::new(),
            },
            &mut payload,
        );
        Frame { payload, ..frame }
            .encode(&mut self.response)
            .unwrap();
        Ok(())
    }
    fn read_chunk(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        // Exercise acknowledgements split across reads as on the guest socket.
        let count = bytes.len().min(self.response.len()).min(7);
        bytes[..count].copy_from_slice(&self.response[..count]);
        self.response.drain(..count);
        Ok(count)
    }
}

#[test]
fn firmware_upload_preserves_bytes_and_stops_on_rejected_chunk() {
    let manifest = vec![0x51; 1024];
    let artifact: Vec<_> = (0..1024 * 1024 + 19).map(|i| (i % 251) as u8).collect();
    for reject_chunk in [None, Some(3)] {
        let received = Arc::new(Mutex::new(Received::default()));
        let mut client = DebugClient::new(Peer {
            received: received.clone(),
            response: Vec::new(),
            reject_chunk,
        });
        let result = client.upload_update(u64::MAX, &manifest, &artifact);
        let received = received.lock().unwrap();
        if reject_chunk.is_some() {
            assert!(result.is_err());
            assert_eq!(received.chunks, 3);
            assert!(!received.committed);
        } else {
            result.unwrap();
            assert!(received.committed);
            assert_eq!(received.streams[0], manifest);
            assert_eq!(received.streams[1], artifact);
            assert!(
                received.chunks < 32,
                "firmware must not require 4 KiB round trips"
            );
        }
    }
}
