extern crate alloc;

use bexos_kernel_core::ipc::Capability;
use bexos_userspace::{Channel, Rpc};
use hardware_manager_fidl::{
    DriverLifecyclePrepareStopRequest, DriverLifecyclePrepareStopResponse, FidlDecode, FidlEncode,
    HandleRef, Status, StopReason,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrepareStopResult {
    Ack,
    Rejected(Status),
    PeerClosed,
    InvalidResponse,
}

pub fn prepare_stop(lifecycle: Capability, reason: StopReason) -> PrepareStopResult {
    if lifecycle.object_id == 0 {
        return PrepareStopResult::PeerClosed;
    }
    let request = DriverLifecyclePrepareStopRequest { reason };
    let mut bytes = [0; 64];
    let mut handles = [HandleRef { raw: 0 }; 1];
    let encoded = match request.encode(&mut bytes, &mut handles) {
        Ok(encoded) => encoded,
        Err(_) => return PrepareStopResult::InvalidResponse,
    };
    let reply =
        match Rpc(Channel(lifecycle.object_id)).call_raw(1, &bytes[..encoded.bytes], &[], true) {
            Ok(reply) => reply,
            Err(_) => return PrepareStopResult::PeerClosed,
        };
    let refs = reply
        .handles
        .iter()
        .map(|handle| HandleRef { raw: *handle })
        .collect::<alloc::vec::Vec<_>>();
    match DriverLifecyclePrepareStopResponse::decode(&reply.bytes, &refs) {
        Ok(response) if response.status == Status::Ok => PrepareStopResult::Ack,
        Ok(response) => PrepareStopResult::Rejected(response.status),
        Err(_) => PrepareStopResult::InvalidResponse,
    }
}
