use alloc::string::{String, ToString};
use alloc::vec::Vec;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceBinding {
    pub service: String,
    pub protocol: String,
    pub capability: String,
    pub method_ordinals: Vec<u64>,
    pub permission_values: Vec<String>,
    pub caller_package: Option<String>,
    pub caller_uid: Option<u64>,
    pub caller_foreground: bool,
    pub provider_instance_id: Option<String>,
}

impl ServiceBinding {
    pub fn parse(metadata: &str) -> Option<Self> {
        let mut fields = metadata.split('|');
        let service = fields.next()?;
        let protocol = fields.next()?;
        let capability = fields.next()?;
        let ordinals = fields.next()?;
        let values = fields.next().unwrap_or("");
        let caller_package = fields.next().and_then(empty_to_none).map(str::to_string);
        let caller_uid = match fields.next().and_then(empty_to_none) {
            Some(value) => Some(value.parse().ok()?),
            None => None,
        };
        let caller_foreground = fields
            .next()
            .map(|value| value == "fg" || value == "1")
            .unwrap_or(true);
        let provider_instance_id = fields.next().and_then(empty_to_none).map(str::to_string);
        if fields.next().is_some()
            || service.is_empty()
            || protocol.is_empty()
            || capability.is_empty()
        {
            return None;
        }

        let mut method_ordinals = Vec::new();
        if !ordinals.is_empty() {
            for ordinal in ordinals.split(',') {
                method_ordinals.push(ordinal.parse().ok()?);
            }
        }

        Some(Self {
            service: service.to_string(),
            protocol: protocol.to_string(),
            capability: capability.to_string(),
            method_ordinals,
            permission_values: if values.is_empty() {
                Vec::new()
            } else {
                values.split(',').map(str::to_string).collect()
            },
            caller_package,
            caller_uid,
            caller_foreground,
            provider_instance_id,
        })
    }

    pub fn protocol_is(&self, protocol: &str) -> bool {
        self.protocol == protocol
    }

    pub fn allows(&self, ordinal: u64) -> bool {
        self.method_ordinals.contains(&ordinal)
    }
}

fn empty_to_none(value: &str) -> Option<&str> {
    if value.is_empty() { None } else { Some(value) }
}

#[derive(Clone)]
pub struct BoundServiceEndpoint {
    pub channel: crate::Channel,
    pub allowed_methods: Vec<u64>,
    pub protocol: String,
}

impl BoundServiceEndpoint {
    pub fn new(channel: crate::Channel, allowed_methods: Vec<u64>) -> Self {
        Self::new_with_protocol(channel, allowed_methods, "")
    }

    pub fn new_with_protocol(
        channel: crate::Channel,
        allowed_methods: Vec<u64>,
        protocol: &str,
    ) -> Self {
        Self {
            channel,
            allowed_methods,
            protocol: protocol.to_string(),
        }
    }

    pub fn allows(&self, ordinal: u64) -> bool {
        self.allowed_methods.contains(&ordinal)
    }
}
