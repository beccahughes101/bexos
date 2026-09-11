use super::*;
use time_fidl::{
    FidlDecode, FidlEncode, HandleRef, RtcHardwareReadUtcRequest, RtcHardwareReadUtcResponse,
    RtcHardwareWriteUtcRequest, RtcHardwareWriteUtcResponse, Status,
};

pub(super) fn poll_endpoints(state: &mut Runtime) {
    let endpoints = core::mem::take(&mut state.endpoints);
    for endpoint in endpoints {
        match endpoint.channel.try_recv() {
            Ok(message) => {
                let (ordinal, req) = envelope(&message.bytes);
                let handles: Vec<_> = message
                    .handles
                    .iter()
                    .map(|raw| HandleRef { raw: *raw })
                    .collect();
                match ordinal {
                    1 if endpoint.allows(ordinal) => {
                        reply(endpoint.channel, &read_utc_response(state, req, &handles))
                    }
                    2 if endpoint.allows(ordinal) => {
                        reply(endpoint.channel, &write_utc_response(state, req, &handles))
                    }
                    _ => {}
                }
                state.requests = state.requests.wrapping_add(1);
                state.endpoints.push(endpoint);
            }
            Err(kernel_fidl::Status::ErrPeerClosed) => {}
            Err(_) => state.endpoints.push(endpoint),
        }
    }
}

fn read_utc_response(
    state: &Runtime,
    req: &[u8],
    handles: &[HandleRef],
) -> RtcHardwareReadUtcResponse {
    if RtcHardwareReadUtcRequest::decode(req, handles).is_err() {
        return RtcHardwareReadUtcResponse {
            status: Status::ErrInvalidArgs,
            utc_timestamp_ns: 0,
        };
    }
    let Some(rtc) = state.rtc() else {
        return RtcHardwareReadUtcResponse {
            status: Status::ErrIo,
            utc_timestamp_ns: 0,
        };
    };
    match rtc.read_utc_ns() {
        Ok(utc_timestamp_ns) => RtcHardwareReadUtcResponse {
            status: Status::Ok,
            utc_timestamp_ns,
        },
        Err(_) => RtcHardwareReadUtcResponse {
            status: Status::ErrInvalidArgs,
            utc_timestamp_ns: 0,
        },
    }
}

fn write_utc_response(
    state: &Runtime,
    req: &[u8],
    handles: &[HandleRef],
) -> RtcHardwareWriteUtcResponse {
    let Ok(request) = RtcHardwareWriteUtcRequest::decode(req, handles) else {
        return RtcHardwareWriteUtcResponse {
            status: Status::ErrInvalidArgs,
        };
    };
    let Some(mut rtc) = state.rtc() else {
        return RtcHardwareWriteUtcResponse {
            status: Status::ErrIo,
        };
    };
    let status = match rtc.write_utc_ns(request.utc_timestamp_ns) {
        Ok(()) => Status::Ok,
        Err(_) => Status::ErrInvalidArgs,
    };
    RtcHardwareWriteUtcResponse { status }
}

fn reply<T: FidlEncode>(channel: Channel, response: &T) {
    let mut out = [0; 64];
    let mut out_handles = [HandleRef { raw: 0 }; 1];
    if let Ok(encoded) = response.encode(&mut out, &mut out_handles) {
        let raw: Vec<_> = out_handles[..encoded.handles]
            .iter()
            .map(|handle| handle.raw)
            .collect();
        let _ = channel.send(&out[..encoded.bytes], &raw);
    }
}

fn envelope(bytes: &[u8]) -> (u64, &[u8]) {
    if bytes.len() < 8 {
        return (0, &[]);
    }
    let mut raw = [0; 8];
    raw.copy_from_slice(&bytes[..8]);
    (u64::from_le_bytes(raw), &bytes[8..])
}
