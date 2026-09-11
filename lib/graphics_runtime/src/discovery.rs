//! Owned discovery snapshots for protocol clients. Query at startup or device
//! changes; synchronous discovery never belongs in the steady-state frame loop.
use bexos_userspace::{Channel, ipc::Message};
use bexos_virtio_gpu_protocol::{Capabilities, Capset, Mode};
use graphics_fidl::*;
pub struct DisplayCapabilities {
    pub gpu: Capabilities,
    pub hardware_vsync: bool,
    pub direct_scanout: bool,
}
fn request(channel: &mut Channel, ordinal: u64, q: &impl FidlEncode) -> Result<Message, Status> {
    let mut bytes = [0; 128];
    bytes[..8].copy_from_slice(&ordinal.to_le_bytes());
    let n = q
        .encode(&mut bytes[8..], &mut [])
        .map_err(|_| Status::ErrInvalidArgs)?;
    channel
        .send(&bytes[..8 + n.bytes], &[])
        .map_err(|_| Status::ErrIo)?;
    let message = match channel.recv_with_timeout(2) {
        Ok(m) => m,
        Err(_) => {
            crate::close(&[channel.0]);
            channel.0 = 0;
            return Err(Status::ErrIo);
        }
    };
    if !message.handles.is_empty() {
        crate::close(&message.handles);
        return Err(Status::ErrInvalidArgs);
    }
    Ok(message)
}
pub fn read(channel: &mut Channel) -> Result<DisplayCapabilities, Status> {
    let m = request(channel, 7, &DisplayCoordinatorGetCapabilitiesRequest {})?;
    let reply = DisplayCoordinatorGetCapabilitiesResponse::decode(&m.bytes, &[])
        .map_err(|_| Status::ErrInvalidArgs)?;
    if reply.status != Status::Ok {
        return Err(reply.status);
    }
    let mut out = DisplayCapabilities {
        gpu: Capabilities {
            offered: reply.offered_features,
            negotiated: reply.negotiated_features,
            ..Default::default()
        },
        hardware_vsync: reply.hardware_vsync,
        direct_scanout: reply.direct_scanout,
    };
    for i in 0..reply.capsets.len() {
        let c = reply.capsets.get(i).map_err(|_| Status::ErrInvalidArgs)?;
        out.gpu.capsets.push(Capset {
            id: c.id,
            version: c.max_version,
            size: c.max_size,
        });
    }
    let m = request(channel, 8, &DisplayCoordinatorGetDisplayModesRequest {})?;
    let reply = DisplayCoordinatorGetDisplayModesResponse::decode(&m.bytes, &[])
        .map_err(|_| Status::ErrInvalidArgs)?;
    if reply.status != Status::Ok {
        return Err(reply.status);
    }
    for i in 0..reply.modes.len() {
        let m = reply.modes.get(i).map_err(|_| Status::ErrInvalidArgs)?;
        out.gpu.modes.push(Mode {
            scanout: m.scanout,
            x: m.x,
            y: m.y,
            width: m.width,
            height: m.height,
            enabled: m.enabled,
            flags: m.flags,
        });
    }
    out.gpu.validate().map_err(|_| Status::ErrInvalidArgs)?;
    Ok(out)
}
pub fn capset(channel: &mut Channel, id: u32, version: u32) -> Result<Vec<u8>, Status> {
    let m = request(
        channel,
        9,
        &DisplayCoordinatorGetCapsetRequest { id, version },
    )?;
    let reply = DisplayCoordinatorGetCapsetResponse::decode(&m.bytes, &[])
        .map_err(|_| Status::ErrInvalidArgs)?;
    if reply.status != Status::Ok {
        return Err(reply.status);
    }
    Ok(reply.data.to_vec())
}
