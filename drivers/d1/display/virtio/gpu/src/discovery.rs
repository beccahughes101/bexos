//! Device capability reads; offered functionality is distinct from negotiated
//! transport features and from the backends the compositor can actually use.
use crate::hardware::Hardware;
use bexos_virtio_gpu_protocol::{Capabilities, MAX_CAPSETS};
use kernel_fidl::Status;
use virtio_drivers::transport::Transport;
impl Hardware {
    pub fn discover(&mut self) -> Result<(), Status> {
        let scanouts = self
            .transport
            .read_config_space::<u32>(8)
            .map_err(|_| Status::ErrInvalidArgs)?;
        let count = self
            .transport
            .read_config_space::<u32>(12)
            .map_err(|_| Status::ErrInvalidArgs)?;
        if count > MAX_CAPSETS as u32 {
            return Err(Status::ErrInvalidArgs);
        }
        let mut capabilities = Capabilities {
            offered: self.transport.read_device_features(),
            negotiated: bexos_virtio_gpu_protocol::transport::negotiated_features(
                self.transport.read_device_features(),
            ),
            ..Default::default()
        };
        let len = self.request_response(0x100, &[], 0x1101)?;
        capabilities
            .parse_modes(&unsafe { self.command.bytes() }[4096..4096 + len], scanouts)
            .map_err(|_| Status::ErrInvalidArgs)?;
        for index in 0..count {
            let mut payload = [0; 8];
            payload[..4].copy_from_slice(&index.to_le_bytes());
            let len = self.request_response(0x108, &payload, 0x1102)?;
            capabilities
                .add_capset(&unsafe { self.command.bytes() }[4096..4096 + len])
                .map_err(|_| Status::ErrInvalidArgs)?;
        }
        bexos_userspace::log(&format!(
            "virtio-gpu: discovered scanouts={} capsets={} venus_offered={} negotiated={:#x}\n",
            capabilities.modes.len(),
            capabilities.capsets.len(),
            capabilities.venus_offered(),
            capabilities.negotiated
        ));
        self.capabilities = capabilities;
        Ok(())
    }
    pub fn capset(&mut self, id: u32, version: u32) -> Result<&[u8], Status> {
        let info = self
            .capabilities
            .capsets
            .iter()
            .find(|c| c.id == id && version <= c.version)
            .ok_or(Status::ErrInvalidArgs)?;
        let size = info.size as usize;
        if size > 4096 - 24 {
            return Err(Status::ErrNoMemory);
        }
        let mut payload = [0; 8];
        payload[..4].copy_from_slice(&id.to_le_bytes());
        payload[4..].copy_from_slice(&version.to_le_bytes());
        let len = self.request_response(0x109, &payload, 0x1103)?;
        if len != 24 + size {
            return Err(Status::ErrInvalidArgs);
        }
        Ok(&unsafe { self.command.bytes() }[4096 + 24..4096 + len])
    }
}
pub fn handle(
    d: &mut crate::state::Display,
    channel: bexos_userspace::Channel,
    ordinal: u64,
    bytes: &[u8],
    handles: &[u64],
) -> bool {
    use bexos_graphics_runtime::reply;
    use g::FidlDecode;
    use graphics_fidl as g;
    if !(7..=9).contains(&ordinal) {
        return false;
    }
    let ready = if d.hardware.as_ref().is_some_and(|h| h.healthy) {
        g::Status::Ok
    } else {
        g::Status::ErrIo
    };
    match ordinal {
        7 => {
            let valid = handles.is_empty()
                && g::DisplayCoordinatorGetCapabilitiesRequest::decode(bytes, &[]).is_ok();
            let status = if valid {
                ready
            } else {
                g::Status::ErrInvalidArgs
            };
            let mut caps = [g::GpuCapsetInfo {
                id: 0,
                max_version: 0,
                max_size: 0,
            }; 16];
            let hardware = d.hardware.as_ref();
            let count = if status == g::Status::Ok {
                hardware.map_or(0, |h| h.capabilities.capsets.len())
            } else {
                0
            };
            if let Some(h) = hardware {
                for (out, c) in caps.iter_mut().zip(&h.capabilities.capsets).take(count) {
                    *out = g::GpuCapsetInfo {
                        id: c.id,
                        max_version: c.version,
                        max_size: c.size,
                    };
                }
            }
            reply(
                channel,
                &g::DisplayCoordinatorGetCapabilitiesResponse {
                    status,
                    offered_features: hardware.map_or(0, |h| h.capabilities.offered),
                    negotiated_features: hardware.map_or(0, |h| h.capabilities.negotiated),
                    hardware_vsync: false,
                    direct_scanout: status == g::Status::Ok,
                    capsets: g::WireVector::from_slice(&caps[..count]),
                },
            );
        }
        8 => {
            let valid = handles.is_empty()
                && g::DisplayCoordinatorGetDisplayModesRequest::decode(bytes, &[]).is_ok();
            let status = if valid {
                ready
            } else {
                g::Status::ErrInvalidArgs
            };
            let mut modes = [g::DisplayMode {
                scanout: 0,
                x: 0,
                y: 0,
                width: 0,
                height: 0,
                enabled: false,
                flags: 0,
            }; 16];
            let count = if status == g::Status::Ok {
                d.hardware
                    .as_ref()
                    .map_or(0, |h| h.capabilities.modes.len())
            } else {
                0
            };
            if let Some(h) = &d.hardware {
                for (out, m) in modes.iter_mut().zip(&h.capabilities.modes).take(count) {
                    *out = g::DisplayMode {
                        scanout: m.scanout,
                        x: m.x,
                        y: m.y,
                        width: m.width,
                        height: m.height,
                        enabled: m.enabled,
                        flags: m.flags,
                    };
                }
            }
            reply(
                channel,
                &g::DisplayCoordinatorGetDisplayModesResponse {
                    status,
                    modes: g::WireVector::from_slice(&modes[..count]),
                },
            );
        }
        9 => {
            let result = read_capset(d, bytes, handles);
            let response = match result {
                Ok(data) => g::DisplayCoordinatorGetCapsetResponse {
                    status: g::Status::Ok,
                    data,
                },
                Err(status) => g::DisplayCoordinatorGetCapsetResponse { status, data: &[] },
            };
            bexos_graphics_runtime::try_reply_in(channel, &response, &mut [0; 8192]);
        }
        _ => unreachable!(),
    }
    true
}

fn read_capset<'a>(
    d: &'a mut crate::state::Display,
    bytes: &[u8],
    handles: &[u64],
) -> Result<&'a [u8], graphics_fidl::Status> {
    use g::FidlDecode;
    use graphics_fidl as g;
    if !handles.is_empty() {
        return Err(g::Status::ErrInvalidArgs);
    }
    let q = g::DisplayCoordinatorGetCapsetRequest::decode(bytes, &[])
        .map_err(|_| g::Status::ErrInvalidArgs)?;
    let h = d
        .hardware
        .as_mut()
        .filter(|h| h.healthy)
        .ok_or(g::Status::ErrIo)?;
    h.capset(q.id, q.version).map_err(|e| match e {
        Status::ErrNoMemory => g::Status::ErrNoMemory,
        Status::ErrInvalidArgs => g::Status::ErrInvalidArgs,
        _ => g::Status::ErrIo,
    })
}
