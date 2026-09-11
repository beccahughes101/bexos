pub mod registry;
pub mod state;
mod wait;
use alloc::format;
use bexos_d1_pci::{EcamConfigSpace, PciRootBus, RootBusConfig, node_id};
use bexos_userspace::{
    Memory,
    live_migration::{Source, now_ms},
    service_binding::ServiceBinding,
};
use state::{Pending, Runtime};
fn input(device: &bexos_d1_pci::PciDevice) -> bool {
    device.vendor_id == 0x1af4 && device.device_id == 0x1052
}
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
            } else if accepted {
                s.known.retain(|node| *node != p.node);
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
        RootBusConfig::qemu_virt(),
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
    if let Some(node) = s.known.iter().copied().find(|node| {
        !devices
            .iter()
            .any(|d| input(d) && node_id(d.address) == *node)
    }) {
        if registry::unregister(s.registry, node).is_ok() {
            s.pending = Some(Pending { node, added: false });
        }
    } else if s.known.len() < 16 {
        if let Some(device) = devices
            .into_iter()
            .find(|d| input(d) && !s.known.contains(&node_id(d.address)))
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
                    if registry::register(s.registry, &node).is_ok() {
                        s.pending = Some(Pending {
                            node: node.node_id,
                            added: true,
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
        if poll(&mut s, source.draining()) {
            source.changed(1);
        }
        wait::idle(&s, source.draining());
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
