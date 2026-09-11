use alloc::{collections::BTreeMap, vec::Vec};
use bexos_usb_host::{
    ClassPolicy, InterfaceClass, PolicyDecision,
    descriptor::{Configuration, DeviceDescriptor, EndpointDescriptor},
};
use usb_host_fidl::{InterfaceInfo, UsbSpeed};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Interface {
    pub id: u64,
    pub device_id: u64,
    pub number: u8,
    pub generation: u64,
    pub class: InterfaceClass,
    pub class_code: u8,
    pub subclass: u8,
    pub protocol: u8,
    pub speed: UsbSpeed,
    pub endpoints: Vec<EndpointDescriptor>,
    pub owner: Option<u64>,
    pub channel: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub id: u64,
    pub parent_id: u64,
    pub port: u8,
    pub generation: u64,
    pub speed: UsbSpeed,
    pub descriptor: DeviceDescriptor,
    pub configuration_value: u8,
    pub interfaces: Vec<Interface>,
}

#[derive(Clone, Debug, Default)]
pub struct Topology {
    pub devices: BTreeMap<u64, Device>,
    pub next_device_id: u64,
    pub next_interface_id: u64,
    pub generation: u64,
}

impl Topology {
    pub fn admit_configuration(
        &mut self,
        parent_id: u64,
        port: u8,
        speed: UsbSpeed,
        device: DeviceDescriptor,
        configuration: &Configuration,
        policy: ClassPolicy,
    ) -> Result<u64, ()> {
        if !policy.admits_composite(configuration) {
            return Err(());
        }
        self.generation = self.generation.wrapping_add(1).max(1);
        self.next_device_id = self.next_device_id.wrapping_add(1).max(1);
        let id = self.next_device_id;
        let mut interfaces = Vec::new();
        for interface in &configuration.interfaces {
            let class = match policy.decide_interface(interface) {
                PolicyDecision::Admit(class) => class,
                PolicyDecision::Reject => return Err(()),
            };
            self.next_interface_id = self.next_interface_id.wrapping_add(1).max(1);
            interfaces.push(Interface {
                id: self.next_interface_id,
                device_id: id,
                number: interface.number,
                generation: self.generation,
                class,
                class_code: interface.class_code,
                subclass: interface.subclass,
                protocol: interface.protocol,
                speed,
                endpoints: interface.endpoints.clone(),
                owner: None,
                channel: None,
            });
        }
        self.devices.insert(
            id,
            Device {
                id,
                parent_id,
                port,
                generation: self.generation,
                speed,
                descriptor: device,
                configuration_value: configuration.value,
                interfaces,
            },
        );
        Ok(id)
    }

    pub fn remove_subtree(&mut self, device_id: u64) -> Vec<u64> {
        let mut removed = Vec::new();
        let children: Vec<_> = self
            .devices
            .values()
            .filter(|device| device.parent_id == device_id)
            .map(|device| device.id)
            .collect();
        for child in children {
            removed.extend(self.remove_subtree(child));
        }
        if self.devices.remove(&device_id).is_some() {
            removed.push(device_id);
            self.generation = self.generation.wrapping_add(1).max(1);
        }
        removed
    }

    pub fn interface_mut(&mut self, device_id: u64, number: u8) -> Option<&mut Interface> {
        self.devices
            .get_mut(&device_id)?
            .interfaces
            .iter_mut()
            .find(|interface| interface.number == number)
    }

    pub fn wire_info(interface: &Interface) -> InterfaceInfo<'static> {
        InterfaceInfo {
            device_id: interface.device_id,
            interface_id: interface.id,
            generation: interface.generation,
            class_code: interface.class_code,
            subclass: interface.subclass,
            protocol: interface.protocol,
            speed: interface.speed,
            endpoints: usb_host_fidl::WireVector::from_slice(&[]),
        }
    }
}
