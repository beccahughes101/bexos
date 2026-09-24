extern crate alloc;

use bexos_kernel_core::ipc::Capability;
use bexos_userspace::{Channel, Rpc};
use hardware_manager_fidl::{
    DriverLifecyclePrepareStopRequest, DriverLifecyclePrepareStopResponse, FidlDecode, FidlEncode,
    HandleRef, PciDeviceControlResetRequest, PciDeviceControlResetResponse,
    PciDeviceControlSetBusMasterRequest, PciDeviceControlSetBusMasterResponse, Status, StopReason,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrepareStopResult {
    Ack,
    Rejected(Status),
    PeerClosed,
    InvalidResponse,
}

pub fn set_pci_bus_master(control: Capability, enabled: bool) -> Result<(), Status> {
    let request = PciDeviceControlSetBusMasterRequest { enabled };
    let mut bytes = [0; 32];
    let mut handles = [HandleRef { raw: 0 }; 1];
    let encoded = request
        .encode(&mut bytes, &mut handles)
        .map_err(|_| Status::ErrInvalidArgs)?;
    let reply = Rpc(Channel(control.object_id))
        .call_raw(1, &bytes[..encoded.bytes], &[], true)
        .map_err(|_| Status::ErrPeerClosed)?;
    let response = PciDeviceControlSetBusMasterResponse::decode(&reply.bytes, &[])
        .map_err(|_| Status::ErrInvalidArgs)?;
    if response.status == Status::Ok {
        Ok(())
    } else {
        Err(response.status)
    }
}

pub fn reset_pci(control: Capability) -> Result<(), Status> {
    let mut bytes = [0; 16];
    let encoded = PciDeviceControlResetRequest {}
        .encode(&mut bytes, &mut [])
        .map_err(|_| Status::ErrInvalidArgs)?;
    let reply = Rpc(Channel(control.object_id))
        .call_raw(2, &bytes[..encoded.bytes], &[], true)
        .map_err(|_| Status::ErrPeerClosed)?;
    let response = PciDeviceControlResetResponse::decode(&reply.bytes, &[])
        .map_err(|_| Status::ErrInvalidArgs)?;
    if response.status == Status::Ok {
        Ok(())
    } else {
        Err(response.status)
    }
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
