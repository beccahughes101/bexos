use bexos_usb_host::descriptor::{
    Configuration, DeviceDescriptor, EndpointDescriptor, InterfaceDescriptor,
};
use bexos_userspace::{Channel, live_migration::State};
use usb_host_fidl::UsbSpeed;

#[test]
fn topology_claims_and_watchers_survive_replacement() {
    let mut runtime = bexos_usbd::Runtime::new(Channel(1), Some(Channel(2)));
    let configuration = Configuration {
        value: 1,
        interfaces: vec![InterfaceDescriptor {
            number: 0,
            class_code: 3,
            subclass: 1,
            protocol: 1,
            endpoints: vec![EndpointDescriptor {
                address: 0x81,
                attributes: 3,
                max_packet_size: 8,
                interval: 10,
            }],
            ..Default::default()
        }],
        ..Default::default()
    };
    runtime
        .topology
        .admit_configuration(
            0,
            1,
            UsbSpeed::Full,
            DeviceDescriptor {
                vendor_id: 0x46d,
                product_id: 1,
                configurations: 1,
                ..Default::default()
            },
            &configuration,
            runtime.policy,
        )
        .unwrap();
    runtime.topology_watchers.push(Channel(7));
    runtime.claims.insert(1, 99);
    let mut new = bexos_usbd::Runtime::empty();
    for key in runtime.keys() {
        new.adopt_record(key, runtime.encode_record(key).unwrap().as_deref())
            .unwrap();
    }
    new.finish_adoption().unwrap();
    assert_eq!(new.topology.devices.len(), 1);
    assert_eq!(new.topology_watchers[0].0, 7);
    assert_eq!(new.claims[&1], 99);
}
