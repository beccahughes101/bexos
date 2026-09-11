use alloc::vec::Vec;

pub const DT_DEVICE: u8 = 1;
pub const DT_CONFIGURATION: u8 = 2;
pub const DT_INTERFACE: u8 = 4;
pub const DT_ENDPOINT: u8 = 5;
pub const MAX_DESCRIPTOR_BYTES: usize = 4096;
pub const MAX_INTERFACES: usize = 16;
pub const MAX_ENDPOINTS_PER_INTERFACE: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DescriptorError {
    Truncated,
    BadLength,
    Unsupported,
    Capacity,
    Duplicate,
    Missing,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DeviceDescriptor {
    pub usb_bcd: u16,
    pub class_code: u8,
    pub subclass: u8,
    pub protocol: u8,
    pub max_packet_size0: u8,
    pub vendor_id: u16,
    pub product_id: u16,
    pub device_bcd: u16,
    pub configurations: u8,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EndpointDescriptor {
    pub address: u8,
    pub attributes: u8,
    pub max_packet_size: u16,
    pub interval: u8,
}

impl EndpointDescriptor {
    pub fn endpoint_number(self) -> u8 {
        self.address & 0x0f
    }
    pub fn direction_in(self) -> bool {
        self.address & 0x80 != 0
    }
    pub fn transfer_type(self) -> u8 {
        self.attributes & 0x03
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InterfaceDescriptor {
    pub number: u8,
    pub alternate_setting: u8,
    pub class_code: u8,
    pub subclass: u8,
    pub protocol: u8,
    pub endpoints: Vec<EndpointDescriptor>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Configuration {
    pub value: u8,
    pub attributes: u8,
    pub max_power_ma: u16,
    pub interfaces: Vec<InterfaceDescriptor>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ParsedDescriptors {
    pub device: Option<DeviceDescriptor>,
    pub configuration: Option<Configuration>,
}

pub fn parse_device(bytes: &[u8]) -> Result<DeviceDescriptor, DescriptorError> {
    if bytes.len() < 18 {
        return Err(DescriptorError::Truncated);
    }
    if bytes[0] != 18 || bytes[1] != DT_DEVICE {
        return Err(DescriptorError::BadLength);
    }
    Ok(DeviceDescriptor {
        usb_bcd: le16(bytes, 2)?,
        class_code: bytes[4],
        subclass: bytes[5],
        protocol: bytes[6],
        max_packet_size0: bytes[7],
        vendor_id: le16(bytes, 8)?,
        product_id: le16(bytes, 10)?,
        device_bcd: le16(bytes, 12)?,
        configurations: bytes[17],
    })
}

pub fn parse_configuration(bytes: &[u8]) -> Result<Configuration, DescriptorError> {
    if bytes.len() < 9 {
        return Err(DescriptorError::Truncated);
    }
    if bytes.len() > MAX_DESCRIPTOR_BYTES {
        return Err(DescriptorError::Capacity);
    }
    if bytes[0] != 9 || bytes[1] != DT_CONFIGURATION {
        return Err(DescriptorError::BadLength);
    }
    let total = usize::from(le16(bytes, 2)?);
    if total < 9 || total > bytes.len() || total > MAX_DESCRIPTOR_BYTES {
        return Err(DescriptorError::Truncated);
    }
    let mut out = Configuration {
        value: bytes[5],
        attributes: bytes[7],
        max_power_ma: u16::from(bytes[8]) * 2,
        interfaces: Vec::new(),
    };
    let mut offset = 9;
    let mut current: Option<InterfaceDescriptor> = None;
    while offset < total {
        let len = *bytes.get(offset).ok_or(DescriptorError::Truncated)? as usize;
        let ty = *bytes.get(offset + 1).ok_or(DescriptorError::Truncated)?;
        if len < 2 || offset.checked_add(len).is_none_or(|end| end > total) {
            return Err(DescriptorError::BadLength);
        }
        let d = &bytes[offset..offset + len];
        match ty {
            DT_INTERFACE => {
                if len < 9 {
                    return Err(DescriptorError::BadLength);
                }
                if let Some(interface) = current.take() {
                    push_interface(&mut out, interface)?;
                }
                current = Some(InterfaceDescriptor {
                    number: d[2],
                    alternate_setting: d[3],
                    class_code: d[5],
                    subclass: d[6],
                    protocol: d[7],
                    endpoints: Vec::new(),
                });
            }
            DT_ENDPOINT => {
                if len < 7 {
                    return Err(DescriptorError::BadLength);
                }
                let interface = current.as_mut().ok_or(DescriptorError::Missing)?;
                if interface.endpoints.len() >= MAX_ENDPOINTS_PER_INTERFACE {
                    return Err(DescriptorError::Capacity);
                }
                let endpoint = EndpointDescriptor {
                    address: d[2],
                    attributes: d[3],
                    max_packet_size: le16(d, 4)?,
                    interval: d[6],
                };
                if endpoint.endpoint_number() == 0
                    || endpoint.endpoint_number() > 15
                    || interface
                        .endpoints
                        .iter()
                        .any(|existing| existing.address == endpoint.address)
                {
                    return Err(DescriptorError::Duplicate);
                }
                interface.endpoints.push(endpoint);
            }
            _ => {}
        }
        offset += len;
    }
    if let Some(interface) = current.take() {
        push_interface(&mut out, interface)?;
    }
    if out.interfaces.is_empty() {
        return Err(DescriptorError::Missing);
    }
    Ok(out)
}

fn push_interface(
    configuration: &mut Configuration,
    interface: InterfaceDescriptor,
) -> Result<(), DescriptorError> {
    if configuration.interfaces.len() >= MAX_INTERFACES {
        return Err(DescriptorError::Capacity);
    }
    if interface.alternate_setting != 0 {
        return Err(DescriptorError::Unsupported);
    }
    if configuration
        .interfaces
        .iter()
        .any(|existing| existing.number == interface.number)
    {
        return Err(DescriptorError::Duplicate);
    }
    configuration.interfaces.push(interface);
    Ok(())
}

fn le16(bytes: &[u8], offset: usize) -> Result<u16, DescriptorError> {
    Ok(u16::from_le_bytes(
        bytes
            .get(offset..offset + 2)
            .ok_or(DescriptorError::Truncated)?
            .try_into()
            .unwrap(),
    ))
}
