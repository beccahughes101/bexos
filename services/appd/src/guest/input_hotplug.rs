//! Runtime requests on the private endpoint handed to the PCI registrar at
//! bootstrap. Input binding uses the installed archive and normal driver policy.
use super::{state::AppdState, *};
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use hardware_manager_fidl::{self as h, FidlDecode, FidlEncode};
pub const KEY: u64 = 11;
const PACKAGE: &str = "bexos.driver.input.virtio";
fn valid_node(node: u64) -> bool {
    node >> 16 < 16 && (node >> 8) & 255 < 32 && node & 255 < 8
}
#[derive(Default)]
pub struct Hotplug {
    pub registry: u64,
    pub bind_pending: bool,
    pub removing: Option<(u64, u64)>,
}
impl Hotplug {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Encoder::new();
        w.word(1);
        w.word(self.registry);
        w.word(self.bind_pending as u64);
        w.word(self.removing.is_some() as u64);
        if let Some((node, deadline)) = self.removing {
            w.word(node);
            w.word(deadline);
        }
        w.finish()
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = Decoder::new(bytes);
        if r.word()? != 1 {
            return Err(Error::UnsupportedVersion);
        }
        let state = Self {
            registry: r.word()?,
            bind_pending: r.flag()?,
            removing: if r.flag()? {
                Some((r.word()?, r.word()?))
            } else {
                None
            },
        };
        r.finish()?;
        if state.registry == 0
            || state
                .removing
                .is_some_and(|(node, deadline)| !valid_node(node) || deadline == 0)
        {
            return Err(Error::InvalidData);
        }
        Ok(state)
    }
}
fn input(node: &crate::RegisteredDeviceNode) -> bool {
    node.info.bus == crate::BusType::Pci
        && [("pci.vendor_id", 0x1af4), ("pci.device_id", 0x1052)]
            .iter()
            .all(|(key, value)| {
                node.info
                    .properties
                    .iter()
                    .any(|p| p.key == *key && p.value == *value)
            })
}
fn binding(state: &crate::DeviceNodeState) -> Option<&crate::DriverBinding> {
    match state {
        crate::DeviceNodeState::Binding(b)
        | crate::DeviceNodeState::Active(b)
        | crate::DeviceNodeState::Quiescing(b)
        | crate::DeviceNodeState::Suspended(b) => Some(b),
        _ => None,
    }
}
fn reply(channel: u64, status: h::Status) {
    let mut bytes = [0; 16];
    if let Ok(n) =
        (h::DeviceRegistryRegisterDeviceNodeResponse { status }).encode(&mut bytes, &mut [])
    {
        let _ = Channel(channel).send(&bytes[..n.bytes], &[]);
    }
}
fn close(handles: &[u64]) {
    for handle in handles {
        if *handle != 0 {
            let _ = Memory::close(*handle);
        }
    }
}
pub fn poll(
    state: &mut AppdState,
    kernel: &mut KernelFidlOps<KernelTransport, KernelTransport, KernelTransport>,
    source: &mut Source,
) {
    if state.input_hotplug.registry == 0 || source.active() {
        return;
    }
    if let Some((node, deadline)) = state.input_hotplug.removing {
        let process = state
            .devices
            .nodes()
            .iter()
            .find(|n| n.info.node_id == node)
            .and_then(|n| binding(&n.state))
            .and_then(|b| b.process_handle.map(|p| p.object_id));
        if process.is_some_and(|p| !super::process_terminated(p)) {
            if bexos_graphics_runtime::now_us() >= deadline {
                state.input_hotplug.removing = None;
                reply(state.input_hotplug.registry, h::Status::ErrTimedOut);
            }
            return;
        }
        let removed = state
            .devices
            .unregister_device_node_post_order(node)
            .unwrap_or_default();
        let mut handles = alloc::collections::BTreeSet::new();
        for n in removed {
            state
                .broker
                .remove_instance(PACKAGE, &n.info.node_id.to_string());
            handles.extend(n.resources.iter().map(|r| r.capability.object_id));
            if let Some(b) = binding(&n.state) {
                if let Some(p) = b.process_handle {
                    handles.insert(p.object_id);
                }
                if let Some(c) = b.manager_channel {
                    handles.insert(c.object_id);
                }
                if let Some(c) = b.lifecycle_channel {
                    handles.insert(c.object_id);
                }
                if let Some(manager) = b.manager_channel {
                    state.services.retain(|s| {
                        if s.manager != manager.object_id {
                            return true;
                        }
                        handles.extend([
                            s.manager,
                            s.process_handle,
                            s.space_handle,
                            s.thread_handle,
                            s.migration,
                            s.archive,
                        ]);
                        false
                    });
                }
            }
        }
        for handle in handles {
            if handle != 0 {
                let _ = Memory::close(handle);
            }
        }
        state.input_hotplug.removing = None;
        reply(state.input_hotplug.registry, h::Status::Ok);
        log(&format!("appd: input device removed node={node}\n"));
        return;
    }
    if let Ok(message) = Channel(state.input_hotplug.registry).try_recv() {
        let ordinal = message
            .bytes
            .get(..8)
            .map(|b| u64::from_le_bytes(b.try_into().unwrap()))
            .unwrap_or(0);
        let bytes = message.bytes.get(8..).unwrap_or(&[]);
        let refs: Vec<_> = message
            .handles
            .iter()
            .map(|h| h::HandleRef { raw: *h })
            .collect();
        if ordinal == 1 {
            let request = h::DeviceRegistryRegisterDeviceNodeRequest::decode(bytes, &refs);
            let mut accepted = false;
            if let Ok(request) = request {
                let property = |key| {
                    (0..request.info.properties.len())
                        .filter_map(|i| request.info.properties.get(i).ok())
                        .find(|p| p.key == key)
                        .map(|p| p.value)
                };
                let node = request.info.node_id;
                let valid = request.info.bus == h::BusType::Pci
                    && !request.info.has_parent
                    && valid_node(node)
                    && property("pci.vendor_id") == Some(0x1af4)
                    && property("pci.device_id") == Some(0x1052)
                    && property("pci.bus") == Some((node >> 16) as u32)
                    && property("pci.device") == Some(((node >> 8) & 255) as u32)
                    && property("pci.function") == Some((node & 255) as u32)
                    && state.devices.nodes().iter().filter(|n| input(n)).count() < 16
                    && !state.devices.nodes().iter().any(|n| n.info.node_id == node)
                    && request.resources.len() == message.handles.len()
                    && !message.handles.is_empty();
                if valid {
                    accepted =
                        super::readiness::register_fidl_node(&mut state.devices, request).is_ok();
                }
            }
            if !accepted {
                close(&message.handles);
            }
            state.input_hotplug.bind_pending |= accepted;
            reply(
                state.input_hotplug.registry,
                if accepted {
                    h::Status::Ok
                } else {
                    h::Status::ErrInvalidArgs
                },
            );
        } else if ordinal == 2 && message.handles.is_empty() {
            let request = h::DeviceRegistryUnregisterDeviceNodeRequest::decode(bytes, &[]);
            let decoded = request.is_ok();
            if let Ok(request) = request {
                if let Some(node) = state
                    .devices
                    .nodes()
                    .iter()
                    .find(|n| n.info.node_id == request.node_id && input(n))
                {
                    // The PCI registrar has observed physical absence. There is
                    // no live device DMA to reset; close the process, then its
                    // retained grants only after termination is observable.
                    let process =
                        binding(&node.state).and_then(|b| b.process_handle.map(|p| p.object_id));
                    if let Some(process) = process {
                        if crate::runner::KernelOps::terminate_process(
                            kernel,
                            KernelHandle { raw: process },
                            -1,
                        )
                        .is_err()
                        {
                            reply(state.input_hotplug.registry, h::Status::ErrInvalidArgs);
                            return;
                        }
                    }
                    state.input_hotplug.removing = Some((
                        request.node_id,
                        bexos_graphics_runtime::now_us().saturating_add(2_000_000),
                    ));
                    return;
                }
            }
            // Rejected registrations may later disappear without a registry row.
            // Absence is idempotent. The registrar can retry a timed-out
            // removal without losing ownership of a still-registered device.
            reply(
                state.input_hotplug.registry,
                if decoded {
                    h::Status::Ok
                } else {
                    h::Status::ErrInvalidArgs
                },
            );
        } else {
            close(&message.handles);
            reply(state.input_hotplug.registry, h::Status::ErrInvalidArgs);
        }
    }
    if state.input_hotplug.bind_pending {
        state.input_hotplug.bind_pending = false;
        let mut gate = readiness::Gate::new();
        gate.scened = state
            .services
            .iter()
            .find(|s| s.package == "bexos.service.scened")
            .map(|s| Channel(s.manager));
        gate.traced = state
            .services
            .iter()
            .find(|s| s.package == "bexos.service.traced")
            .map(|s| Channel(s.manager));
        gate.inputs = state
            .services
            .iter()
            .filter(|s| s.package == PACKAGE)
            .map(|s| Channel(s.manager))
            .collect();
        gate.registry = core::mem::take(&mut state.devices);
        gate.services = core::mem::take(&mut state.services);
        let result = super::bind_preinstalled_drivers(
            state.vfsd,
            &[PACKAGE.into()],
            &state.registry,
            &state.config,
            kernel,
            &mut state.broker,
            &mut gate,
            &AppdWaveOrchestrator::new(),
        );
        state.devices = gate.registry;
        state.services = gate.services;
        if let Err(error) = result {
            log(&format!("appd: input hotplug bind failed {error}\n"));
        } else {
            log("appd: runtime input discovery complete\n");
        }
    }
}
