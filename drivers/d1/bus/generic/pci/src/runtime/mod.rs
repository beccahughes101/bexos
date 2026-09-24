pub mod registry;
pub mod state;
mod wait;
use alloc::format;
use bexos_d1_pci::{EcamConfigSpace, PciRootBus, node_id};
use bexos_userspace::{
    Memory,
    live_migration::{Source, now_ms},
    service_binding::ServiceBinding,
};
use state::{DeviceControl, Pending, Runtime};
fn poll(s: &mut Runtime, draining: bool) -> bool {
    if let Some(p) = &s.pending {
        let result = s.registry.try_recv();
        if matches!(result, Err(kernel_fidl::Status::ErrPeerClosed)) {
            s.pending = None;
            bexos_userspace::log("pci: registry peer closed; hotplug unavailable\n");
            return true;
        }
        if let Ok(message) = result {
            let accepted = registry::response(message).is_ok();
            if p.added {
                // A rejected registration is remembered until removal: never
                // repeatedly relocate the BARs of a rejected new endpoint.
                s.known.push(p.node);
                if accepted {
                    if let Some(control) = p.control {
                        s.device_controls.push(DeviceControl {
                            node: p.node,
                            channel: control,
                            interrupt: p.interrupt.unwrap_or(0),
                        });
                    }
                } else {
                    if let Some(control) = p.control {
                        let _ = Memory::close(control.0);
                    }
                    if let Some(interrupt) = p.interrupt {
                        let _ = Memory::close(interrupt);
                    }
                }
            } else if accepted {
                s.known.retain(|node| *node != p.node);
                if let Some(index) = s
                    .device_controls
                    .iter()
                    .position(|control| control.node == p.node)
                {
                    let control = s.device_controls.remove(index);
                    let _ = Memory::close(control.channel.0);
                    if control.interrupt != 0 {
                        let _ = Memory::close(control.interrupt);
                    }
                }
            }
            bexos_userspace::log(&format!(
                "pci: input node={} added={} accepted={}\n",
                p.node, p.added, accepted
            ));
            s.pending = None;
            return true;
        }
        return false;
    }
    if draining || !s.extended || s.registry.0 == 0 || now_ms() < s.next_poll_ms {
        return false;
    }
    s.next_poll_ms = now_ms().saturating_add(250);
    let (_, va, _) = s.control.mapping.unwrap();
    let mut bus = match PciRootBus::restore(
        unsafe { EcamConfigSpace::new(va as usize) },
        s.root,
        s.cursor,
    ) {
        Ok(bus) => bus,
        Err(_) => return true,
    };
    let mut devices = match bus.discover_bus0() {
        Ok(devices) => devices,
        Err(_) => return true,
    };
    for port in &s.ports {
        match bus.discover_bus(port.secondary) {
            Ok(children) => devices.extend(children),
            Err(_) => return true,
        }
    }
    devices.retain(|device| node_id(device.address) != 0);
    if let Some(node) = s
        .known
        .iter()
        .copied()
        .find(|node| !devices.iter().any(|d| node_id(d.address) == *node))
    {
        if registry::unregister(s.registry, node).is_ok() {
            s.pending = Some(Pending {
                node,
                added: false,
                control: None,
                interrupt: None,
            });
        }
    } else if s.known.len() < 256 {
        if let Some(device) = devices
            .into_iter()
            .find(|d| !s.known.contains(&node_id(d.address)))
        {
            let initialized = if device.address.bus == 0 {
                bus.initialize_device(device)
            } else if let Some(port) = s
                .ports
                .iter_mut()
                .find(|p| p.secondary == device.address.bus)
            {
                bus.initialize_downstream(port, device)
            } else {
                return true;
            };
            match initialized {
                Ok(node) => {
                    s.cursor = bus.allocation_cursor();
                    if let Ok(registration) = registry::register(s.registry, &node) {
                        s.pending = Some(Pending {
                            node: node.node_id,
                            added: true,
                            control: Some(registration.control),
                            interrupt: Some(registration.interrupt),
                        });
                    } else {
                        s.known.push(node.node_id);
                    }
                }
                Err(error) => {
                    s.known.push(node_id(device.address));
                    bexos_userspace::log(&format!("pci: input assignment failed {error:?}\n"));
                }
            }
        }
    }
    true
}
pub fn serve(mut s: Runtime) -> ! {
    let mut source = Source::new(s.control.migration);
    loop {
        if source.poll(&s).is_err() {
            let _ = bexos_userspace::migration::abort();
        }
        if source.quiescing() {
            wait::idle(&s, true);
            continue;
        }
        if let Ok(message) = s.control.manager.try_recv() {
            let binding = core::str::from_utf8(&message.bytes)
                .ok()
                .and_then(ServiceBinding::parse);
            if message.handles.len() == 1
                && s.power.len() < 8
                && binding.is_some_and(|b| b.protocol_is("DevicePowerControl") && b.allows(1))
            {
                s.power.push(bexos_userspace::Channel(message.handles[0]));
            } else {
                for handle in message.handles {
                    let _ = Memory::close(handle);
                }
            }
            s.control.requests = s.control.requests.wrapping_add(1);
            source.changed(0);
            source.changed(1);
        }
        power(&mut s, &mut source);
        device_control(&mut s, &mut source);
        if poll(&mut s, source.draining()) {
            source.changed(1);
        }
        wait::idle(&s, source.draining());
    }
}

fn device_control(s: &mut Runtime, source: &mut Source) {
    use hardware_manager_fidl::*;
    let Some((_, va, _)) = s.control.mapping else {
        return;
    };
    for control in &s.device_controls {
        let Ok(message) = control.channel.try_recv() else {
            continue;
        };
        let ordinal = message
            .bytes
            .get(..8)
            .and_then(|bytes| bytes.try_into().ok())
            .map(u64::from_le_bytes)
            .unwrap_or(0);
        let request = message.bytes.get(8..).unwrap_or(&[]);
        let status = bexos_d1_pci::address_from_node(control.node)
            .ok_or(())
            .and_then(|address| {
                let mut bus = PciRootBus::restore(
                    unsafe { EcamConfigSpace::new(va as usize) },
                    s.root,
                    s.cursor,
                )
                .map_err(|_| ())?;
                match ordinal {
                    1 => PciDeviceControlSetBusMasterRequest::decode(request, &[])
                        .map_err(|_| ())
                        .and_then(|request| {
                            bus.set_bus_master(address, request.enabled).map_err(|_| ())
                        }),
                    2 => PciDeviceControlResetRequest::decode(request, &[])
                        .map_err(|_| ())
                        .and_then(|_| bus.reset_device(address).map_err(|_| ())),
                    3 => PciDeviceControlResumeRequest::decode(request, &[])
                        .map_err(|_| ())
                        .and_then(|_| bus.resume_device(address).map_err(|_| ())),
                    4 => PciDeviceControlConfigureInterruptsRequest::decode(request, &[])
                        .map_err(|_| ())
                        .and_then(|request| {
                            (request.mode == PciInterruptMode::Legacy
                                && request.vector_count == 1
                                && control.interrupt != 0)
                                .then_some(())
                                .ok_or(())
                        }),
                    _ => Err(()),
                }
            })
            .map_or(Status::ErrInvalidArgs, |_| Status::Ok);
        if ordinal == 4 {
            let duplicate = if status == Status::Ok {
                Memory::duplicate(control.interrupt, 1 | 2 | 4 | 32).ok()
            } else {
                None
            };
            let response_status = if duplicate.is_some() {
                Status::Ok
            } else {
                Status::ErrInvalidArgs
            };
            let handles = duplicate
                .map(|raw| [HandleRef { raw }])
                .unwrap_or([HandleRef { raw: 0 }]);
            let interrupts = if duplicate.is_some() {
                &handles[..]
            } else {
                &handles[..0]
            };
            let mut out = [0u8; 64];
            let mut encoded_handles = [HandleRef { raw: 0 }; 1];
            let encoded = PciDeviceControlConfigureInterruptsResponse {
                status: response_status,
                interrupts,
            }
            .encode(&mut out, &mut encoded_handles);
            if let (Ok(encoded), Some(interrupt)) = (encoded, duplicate) {
                if control
                    .channel
                    .send(&out[..encoded.bytes], &[interrupt])
                    .is_err()
                {
                    let _ = Memory::close(interrupt);
                }
            } else if let Ok(encoded) = encoded {
                let _ = control.channel.send(&out[..encoded.bytes], &[]);
            }
            source.changed(1);
            continue;
        }
        let mut out = [0u8; 32];
        let encoded = match ordinal {
            1 => PciDeviceControlSetBusMasterResponse { status }.encode(&mut out, &mut []),
            2 => PciDeviceControlResetResponse { status }.encode(&mut out, &mut []),
            3 => PciDeviceControlResumeResponse { status }.encode(&mut out, &mut []),
            _ => continue,
        };
        if let Ok(encoded) = encoded {
            let _ = control.channel.send(&out[..encoded.bytes], &[]);
        }
        source.changed(1);
    }
}
fn power(s: &mut Runtime, source: &mut Source) {
    use power_fidl::*;
    s.power.retain(|channel| {
        let message = match channel.try_recv() {
            Ok(message) => message,
            Err(kernel_fidl::Status::ErrPeerClosed) => {
                let _ = Memory::close(channel.0);
                source.changed(1);
                return false;
            }
            Err(_) => return true,
        };
        let status = if message.handles.is_empty()
            && message.bytes.get(..8) == Some(1u64.to_le_bytes().as_slice())
        {
            match DevicePowerControlSetPowerStateRequest::decode(&message.bytes[8..], &[]) {
                // Bus power-off is not implemented; never acknowledge a reset
                // while children retain DMA resources across replacement.
                Ok(q) if q.state == DevicePowerState::D0FullPower => Status::Ok,
                _ => Status::ErrInvalidArgs,
            }
        } else {
            Status::ErrInvalidArgs
        };
        for handle in message.handles {
            let _ = Memory::close(handle);
        }
        let mut bytes = [0; 16];
        if let Ok(n) =
            (DevicePowerControlSetPowerStateResponse { status }).encode(&mut bytes, &mut [])
        {
            let _ = channel.send(&bytes[..n.bytes], &[]);
        }
        true
    });
}
