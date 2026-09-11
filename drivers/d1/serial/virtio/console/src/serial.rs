//! Serialized byte-stream requests on the retained virtio queues.
use crate::hardware::Hardware;
use bexos_userspace::Channel;
use serial_fidl::{
    DeviceReadRequest, DeviceReadResponse, DeviceWriteRequest, DeviceWriteResponse, FidlDecode,
    FidlEncode, Status,
};

pub fn request(hardware: &mut Hardware, channel: Channel, message: &[u8]) {
    if message.len() < 8 {
        return;
    }
    let ordinal = u64::from_le_bytes(message[..8].try_into().unwrap());
    let mut encoded = [0; 8192];
    let result = match ordinal {
        2 => {
            let result = DeviceWriteRequest::decode(&message[8..], &[])
                .map_err(|_| ())
                .and_then(|request| {
                    hardware.console.poll_control().map_err(|_| ())?;
                    if !hardware.console.poll_send().map_err(|_| ())? {
                        return Err(());
                    }
                    hardware.console.send(request.bytes).map_err(|_| ())?;
                    Ok(request.bytes.len() as u32)
                });
            DeviceWriteResponse {
                status: if result.is_ok() {
                    Status::Ok
                } else {
                    Status::ErrTimedOut
                },
                bytes_written: result.unwrap_or(0),
            }
            .encode(&mut encoded, &mut [])
        }
        3 => {
            let mut bytes = [0; 4096];
            let result = DeviceReadRequest::decode(&message[8..], &[])
                .map_err(|_| ())
                .and_then(|request| {
                    hardware.console.poll_control().map_err(|_| ())?;
                    hardware
                        .console
                        .receive(&mut bytes[..(request.max_bytes as usize).min(4096)])
                        .map_err(|_| ())
                });
            DeviceReadResponse {
                status: if result.is_ok() {
                    Status::Ok
                } else {
                    Status::ErrTimedOut
                },
                bytes: &bytes[..result.unwrap_or(0)],
            }
            .encode(&mut encoded, &mut [])
        }
        _ => return,
    };
    if let Ok(result) = result {
        let _ = channel.send(&encoded[..result.bytes], &[]);
    }
}
