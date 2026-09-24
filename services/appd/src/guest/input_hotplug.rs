//! Generic runtime device coordinator for the authenticated registrar endpoint.
//! Every bus uses the same registration, binding, post-order removal and
//! recovery path as the boot wave.
use super::{state::AppdState, *};
use alloc::vec;
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use hardware_manager_fidl::{self as h, FidlDecode, FidlEncode};
pub const KEY: u64 = 11;
#[derive(Default)]
pub struct Hotplug {
    pub registry: u64,
    pub bind_pending: bool,
    pub removing: Option<(u64, u64)>,
    pub driver_packages: Vec<String>,
}
impl Hotplug {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Encoder::new();
        w.word(2);
        w.word(self.registry);
        w.word(self.bind_pending as u64);
        w.word(self.removing.is_some() as u64);
        if let Some((node, deadline)) = self.removing {
            w.word(node);
            w.word(deadline);
        }
        w.word(self.driver_packages.len() as u64);
        for package in &self.driver_packages {
            w.text(package);
        }
        w.finish()
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = Decoder::new(bytes);
        let version = r.word()?;
        if !(1..=2).contains(&version) {
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
            driver_packages: if version >= 2 {
                let mut packages = Vec::new();
                for _ in 0..r.count(1024)? {
                    packages.push(r.text(128)?.into());
                }
                packages
            } else {
                vec!["bexos.driver.input.virtio".into()]
            },
        };
        r.finish()?;
        if state.registry == 0
            || state
                .removing
                .is_some_and(|(node, deadline)| node == 0 || deadline == 0)
            || state.driver_packages.len() > 1024
            || state
                .driver_packages
                .iter()
                .any(|package| package.is_empty())
        {
            return Err(Error::InvalidData);
        }
        Ok(state)
    }
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
            if let Some(binding) = binding(&n.state) {
                state
                    .broker
                    .remove_instance(&binding.package_id, &n.info.node_id.to_string());
                state.driver_routes.close_node(n.info.node_id);
            }
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
        log(&format!("appd: device subtree removed node={node}\n"));
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
                let node = request.info.node_id;
                let valid = node != 0
                    && state.devices.nodes().len() < 1024
                    && !state.devices.nodes().iter().any(|n| n.info.node_id == node)
                    && request.resources.len() == message.handles.len()
                    && request.resources.len() <= 16;
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
                    .find(|n| n.info.node_id == request.node_id)
                {
                    let subtree: Vec<u64> = state
                        .devices
                        .nodes()
                        .iter()
                        .filter(|candidate| {
                            candidate.info.node_id == request.node_id
                                || candidate
                                    .info
                                    .topological_path
                                    .starts_with(&alloc::format!("{}/", node.info.topological_path))
                        })
                        .map(|candidate| candidate.info.node_id)
                        .collect();
                    for candidate in state
                        .devices
                        .nodes()
                        .iter()
                        .filter(|candidate| subtree.contains(&candidate.info.node_id))
                    {
                        if let Some(control) = candidate
                            .resources
                            .iter()
                            .find(|resource| {
                                resource.kind == crate::HardwareResourceKind::BusControl
                            })
                            .map(|resource| resource.capability)
                        {
                            let _ = crate::lifecycle::set_pci_bus_master(control, false);
                            let _ = crate::lifecycle::reset_pci(control);
                        }
                    }
                    // The PCI registrar has observed physical absence. There is
                    // no live device DMA to reset; close the process, then its
                    // retained grants only after termination is observable.
                    let processes: alloc::collections::BTreeSet<u64> = state
                        .devices
                        .nodes()
                        .iter()
                        .filter(|candidate| subtree.contains(&candidate.info.node_id))
                        .filter_map(|candidate| {
                            binding(&candidate.state)
                                .and_then(|binding| binding.process_handle)
                                .map(|process| process.object_id)
                        })
                        .collect();
                    for process in processes {
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
        gate.inputs = state.services.iter().map(|s| Channel(s.manager)).collect();
        gate.registry = core::mem::take(&mut state.devices);
        gate.services = core::mem::take(&mut state.services);
        let result = super::bind_preinstalled_drivers(
            state.vfsd,
            &state.input_hotplug.driver_packages.clone(),
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
            log(&format!("appd: runtime driver bind failed {error}\n"));
        } else {
            log("appd: runtime device discovery complete\n");
        }
    }
}
