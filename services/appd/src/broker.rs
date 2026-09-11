use alloc::string::String;
use alloc::vec::Vec;
use bexos_kernel_core::ipc::Capability;
use bexos_kernel_core::kernel_services::{RIGHT_READ, RIGHT_TRANSFER, RIGHT_WRITE};

use crate::checkpoint::AppdSnapshot;
use crate::manifest::{
    ConsumedService, Lifecycle, LinkType, Manifest, Metadata, ServiceActivation, Visibility,
};
use crate::policy::{ClientContext, PermissionDecision, check_permission, permission_scope_name};
use crate::registry::{CapabilityRegistry, PublishedInterface, RegistryError};

/// Rights retained when appd transfers a routed service endpoint.
///
/// Service protocols are request/response channels, so both peers must be
/// able to read requests or replies and write their counterpart. Transfer is
/// required while appd delivers the endpoint to the process.
pub const SERVICE_ENDPOINT_RIGHTS: u32 = RIGHT_TRANSFER | RIGHT_READ | RIGHT_WRITE;
use crate::runner::{KernelError, KernelOps};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct InterfaceQuery<'a> {
    pub protocol: Option<&'a str>,
    pub metadata: &'a [Metadata],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BindError {
    NotFound,
    PermissionDenied,
    WrongLifecycle,
    Registry(RegistryError),
    Kernel(KernelError),
    InvalidFilter,
    InvalidCapability,
    Ambiguous,
}

impl From<RegistryError> for BindError {
    fn from(error: RegistryError) -> Self {
        Self::Registry(error)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AppdBroker {
    registry: CapabilityRegistry,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoundCapability {
    pub caller_package: String,
    pub caller_uid: Option<u64>,
    pub caller_foreground: bool,
    pub provider_package: String,
    pub provider_instance_id: Option<String>,
    pub service_name: String,
    pub protocol: String,
    pub lifecycle: Lifecycle,
    pub capability: String,
    pub permission: Option<String>,
    pub method_ordinals: Vec<u64>,
    pub permission_values: Vec<String>,
    pub provider_manager: Capability,
    pub client_endpoint: Capability,
    pub provider_endpoint: Capability,
    pub activation: ServiceActivation,
    pub provider_process: Option<String>,
    pub idle_timeout_ms: u32,
    pub dormant_provider: bool,
}

impl AppdBroker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn publish_interface(
        &mut self,
        provider_package: impl Into<String>,
        service: crate::manifest::ExposedService,
        endpoint: Capability,
    ) -> Result<(), BindError> {
        self.registry
            .publish_interface(provider_package, service, endpoint)
            .map_err(BindError::Registry)
    }

    pub fn publish_instance_interface(
        &mut self,
        provider_package: impl Into<String>,
        instance_id: impl Into<String>,
        service: crate::manifest::ExposedService,
        endpoint: Capability,
    ) -> Result<(), BindError> {
        self.registry
            .publish_instance_interface(provider_package, instance_id, service, endpoint)
            .map_err(BindError::Registry)
    }

    pub fn publish_dormant_interface(
        &mut self,
        provider_package: impl Into<String>,
        service: crate::manifest::ExposedService,
        provider_process: impl Into<String>,
    ) -> Result<(), BindError> {
        self.registry
            .publish_dormant_interface(provider_package, service, provider_process)
            .map_err(BindError::Registry)
    }

    pub fn update_provider_manager(
        &mut self,
        provider_package: &str,
        process_name: &str,
        endpoint: Capability,
    ) {
        self.registry
            .update_provider_manager(provider_package, process_name, endpoint);
    }

    pub fn update_instance_manager(
        &mut self,
        provider_package: &str,
        instance_id: &str,
        endpoint: Capability,
    ) {
        self.registry
            .update_instance_manager(provider_package, instance_id, endpoint);
    }

    pub fn remove_instance(&mut self, provider_package: &str, instance_id: &str) {
        self.registry.remove_instance(provider_package, instance_id);
    }

    pub fn remove_provider_package(&mut self, provider_package: &str) {
        self.registry.remove_provider_package(provider_package);
    }

    pub fn publish_manifest_services(
        &mut self,
        manifest: &Manifest,
        first_endpoint_id: u64,
    ) -> Result<(), BindError> {
        for (index, service) in manifest.services_exposed.iter().cloned().enumerate() {
            self.publish_interface(
                manifest.package_name.clone(),
                service,
                Capability {
                    object_id: first_endpoint_id + index as u64,
                    rights: 0b11,
                },
            )?;
        }
        Ok(())
    }

    pub fn freeze(&self) -> AppdSnapshot {
        AppdSnapshot::new(self.registry.interfaces().to_vec())
    }

    pub fn restore(snapshot: AppdSnapshot) -> Result<Self, BindError> {
        snapshot.validate_registry_shape()?;
        let mut registry = CapabilityRegistry::new();
        registry.replace_interfaces(snapshot.interfaces)?;
        Ok(Self { registry })
    }

    pub fn get_interfaces(
        &self,
        client: &ClientContext,
        query: InterfaceQuery<'_>,
    ) -> Vec<PublishedInterface> {
        self.registry
            .interfaces()
            .iter()
            .filter(|interface| {
                query
                    .protocol
                    .is_none_or(|protocol| interface.protocol == protocol)
            })
            .filter(|interface| metadata_matches(&interface.metadata, query.metadata))
            .filter(|interface| can_see(interface, client))
            .filter(|interface| can_bind(interface, client))
            .cloned()
            .collect()
    }

    pub fn get_singleton_interface(
        &self,
        client: &ClientContext,
        name: &str,
    ) -> Result<PublishedInterface, BindError> {
        self.get_by_lifecycle(client, name, Lifecycle::Singleton)
    }

    pub fn get_user_scoped_singleton(
        &self,
        client: &ClientContext,
        name: &str,
    ) -> Result<PublishedInterface, BindError> {
        if client.user_id.is_none() {
            return Err(BindError::PermissionDenied);
        }
        self.get_by_lifecycle(client, name, Lifecycle::UserScopedSingleton)
    }

    pub fn bind_consumed_service<K: KernelOps>(
        &self,
        client: &ClientContext,
        consumed: &ConsumedService,
        kernel: &mut K,
    ) -> Result<Vec<BoundCapability>, BindError> {
        let filter = consumed
            .filter
            .as_deref()
            .map(parse_metadata_filter)
            .transpose()?;
        let required = filter.as_slice();
        let interfaces: Vec<_> = self
            .registry
            .interfaces()
            .iter()
            .filter(|interface| interface.name == consumed.name)
            .filter(|interface| metadata_matches(&interface.metadata, required))
            .filter(|interface| can_see(interface, client))
            .filter(|interface| can_bind(interface, client))
            .collect();
        if interfaces.is_empty() {
            return Err(BindError::NotFound);
        }

        let mut bindings = Vec::new();
        for interface in interfaces {
            let capabilities = scoped_capabilities(consumed, interface)?;
            for capability in capabilities {
                if check_permission(capability.permission.as_deref(), client)
                    != PermissionDecision::Allow
                {
                    continue;
                }
                let method_ordinals = scoped_ordinals(consumed, capability)?;
                if method_ordinals.is_empty() {
                    continue;
                }
                let channel = kernel.create_channel().map_err(BindError::Kernel)?;
                bindings.push(BoundCapability {
                    caller_package: client.package_name.clone(),
                    caller_uid: client.user_id,
                    caller_foreground: client.is_foreground,
                    provider_package: interface.provider_package.clone(),
                    provider_instance_id: interface.instance_id.clone(),
                    service_name: interface.name.clone(),
                    protocol: interface.protocol.clone(),
                    lifecycle: interface.lifecycle,
                    capability: capability.capability.clone(),
                    permission: capability.permission.clone(),
                    method_ordinals,
                    permission_values: permission_scope_name(capability.permission.as_deref())
                        .map(|permission| client.granted_values(permission))
                        .unwrap_or_default(),
                    provider_manager: interface.endpoint,
                    client_endpoint: Capability {
                        object_id: channel.local.raw,
                        rights: SERVICE_ENDPOINT_RIGHTS,
                    },
                    provider_endpoint: Capability {
                        object_id: channel.remote.raw,
                        rights: SERVICE_ENDPOINT_RIGHTS,
                    },
                    activation: interface.activation,
                    provider_process: interface.provider_process.clone(),
                    idle_timeout_ms: interface.idle_timeout_ms,
                    dormant_provider: interface.dormant,
                });
            }
        }

        if bindings.is_empty() {
            Err(BindError::PermissionDenied)
        } else {
            Ok(bindings)
        }
    }

    pub fn bind_consumed_capability<K: KernelOps>(
        &self,
        client: &ClientContext,
        consumed: &ConsumedService,
        capability_name: &str,
        kernel: &mut K,
    ) -> Result<BoundCapability, BindError> {
        if capability_name.is_empty() {
            return Err(BindError::InvalidCapability);
        }
        let requested = consumed
            .capabilities
            .iter()
            .filter(|capability| capability.capability == capability_name)
            .collect::<Vec<_>>();
        if requested.len() != 1 {
            return Err(BindError::InvalidCapability);
        }
        if requested[0].methods.is_empty()
            || requested[0]
                .methods
                .iter()
                .any(|method| method.ordinal == 0)
        {
            return Err(BindError::InvalidCapability);
        }

        let filter = consumed
            .filter
            .as_deref()
            .map(parse_metadata_filter)
            .transpose()?;
        let required = filter.as_slice();
        let interfaces = self
            .registry
            .interfaces()
            .iter()
            .filter(|interface| interface.name == consumed.name)
            .filter(|interface| metadata_matches(&interface.metadata, required))
            .filter(|interface| can_see(interface, client))
            .filter(|interface| can_bind(interface, client))
            .filter(|interface| {
                interface
                    .capabilities
                    .iter()
                    .any(|capability| capability.capability == capability_name)
            })
            .collect::<Vec<_>>();
        if interfaces.is_empty() {
            return Err(BindError::NotFound);
        }
        if interfaces.len() > 1 {
            return Err(BindError::Ambiguous);
        }

        let interface = interfaces[0];
        let provider = interface
            .capabilities
            .iter()
            .find(|capability| capability.capability == capability_name)
            .ok_or(BindError::InvalidCapability)?;
        if check_permission(provider.permission.as_deref(), client) != PermissionDecision::Allow {
            return Err(BindError::PermissionDenied);
        }
        let method_ordinals = scoped_ordinals(consumed, provider)?;
        if method_ordinals.is_empty() {
            return Err(BindError::InvalidCapability);
        }
        let channel = kernel.create_channel().map_err(BindError::Kernel)?;
        Ok(BoundCapability {
            caller_package: client.package_name.clone(),
            caller_uid: client.user_id,
            caller_foreground: client.is_foreground,
            provider_package: interface.provider_package.clone(),
            provider_instance_id: interface.instance_id.clone(),
            service_name: interface.name.clone(),
            protocol: interface.protocol.clone(),
            lifecycle: interface.lifecycle,
            capability: provider.capability.clone(),
            permission: provider.permission.clone(),
            method_ordinals,
            permission_values: permission_scope_name(provider.permission.as_deref())
                .map(|permission| client.granted_values(permission))
                .unwrap_or_default(),
            provider_manager: interface.endpoint,
            client_endpoint: Capability {
                object_id: channel.local.raw,
                rights: SERVICE_ENDPOINT_RIGHTS,
            },
            provider_endpoint: Capability {
                object_id: channel.remote.raw,
                rights: SERVICE_ENDPOINT_RIGHTS,
            },
            activation: interface.activation,
            provider_process: interface.provider_process.clone(),
            idle_timeout_ms: interface.idle_timeout_ms,
            dormant_provider: interface.dormant,
        })
    }

    pub fn bind_runtime_capability(
        &self,
        client: &ClientContext,
        consumed: &ConsumedService,
        capability_name: &str,
        provider_endpoint: Capability,
    ) -> Result<BoundCapability, BindError> {
        if provider_endpoint.object_id == 0 {
            return Err(BindError::InvalidCapability);
        }
        let mut binding = self.bind_consumed_capability_shape(client, consumed, capability_name)?;
        binding.provider_endpoint = provider_endpoint;
        binding.client_endpoint = Capability {
            object_id: 0,
            rights: SERVICE_ENDPOINT_RIGHTS,
        };
        Ok(binding)
    }

    fn bind_consumed_capability_shape(
        &self,
        client: &ClientContext,
        consumed: &ConsumedService,
        capability_name: &str,
    ) -> Result<BoundCapability, BindError> {
        if capability_name.is_empty() {
            return Err(BindError::InvalidCapability);
        }
        let requested = consumed
            .capabilities
            .iter()
            .filter(|capability| capability.capability == capability_name)
            .collect::<Vec<_>>();
        if requested.len() != 1 {
            return Err(BindError::InvalidCapability);
        }
        if requested[0].methods.is_empty()
            || requested[0]
                .methods
                .iter()
                .any(|method| method.ordinal == 0)
        {
            return Err(BindError::InvalidCapability);
        }

        let filter = consumed
            .filter
            .as_deref()
            .map(parse_metadata_filter)
            .transpose()?;
        let required = filter.as_slice();
        let interfaces = self
            .registry
            .interfaces()
            .iter()
            .filter(|interface| interface.name == consumed.name)
            .filter(|interface| metadata_matches(&interface.metadata, required))
            .filter(|interface| can_see(interface, client))
            .filter(|interface| can_bind(interface, client))
            .filter(|interface| {
                interface
                    .capabilities
                    .iter()
                    .any(|capability| capability.capability == capability_name)
            })
            .collect::<Vec<_>>();
        if interfaces.is_empty() {
            return Err(BindError::NotFound);
        }
        if interfaces.len() > 1 {
            return Err(BindError::Ambiguous);
        }

        let interface = interfaces[0];
        let provider = interface
            .capabilities
            .iter()
            .find(|capability| capability.capability == capability_name)
            .ok_or(BindError::InvalidCapability)?;
        if check_permission(provider.permission.as_deref(), client) != PermissionDecision::Allow {
            return Err(BindError::PermissionDenied);
        }
        let method_ordinals = scoped_ordinals(consumed, provider)?;
        if method_ordinals.is_empty() {
            return Err(BindError::InvalidCapability);
        }
        Ok(BoundCapability {
            caller_package: client.package_name.clone(),
            caller_uid: client.user_id,
            caller_foreground: client.is_foreground,
            provider_package: interface.provider_package.clone(),
            provider_instance_id: interface.instance_id.clone(),
            service_name: interface.name.clone(),
            protocol: interface.protocol.clone(),
            lifecycle: interface.lifecycle,
            capability: provider.capability.clone(),
            permission: provider.permission.clone(),
            method_ordinals,
            permission_values: permission_scope_name(provider.permission.as_deref())
                .map(|permission| client.granted_values(permission))
                .unwrap_or_default(),
            provider_manager: interface.endpoint,
            client_endpoint: Capability {
                object_id: 0,
                rights: SERVICE_ENDPOINT_RIGHTS,
            },
            provider_endpoint: Capability {
                object_id: 0,
                rights: SERVICE_ENDPOINT_RIGHTS,
            },
            activation: interface.activation,
            provider_process: interface.provider_process.clone(),
            idle_timeout_ms: interface.idle_timeout_ms,
            dormant_provider: interface.dormant,
        })
    }

    fn get_by_lifecycle(
        &self,
        client: &ClientContext,
        name: &str,
        lifecycle: Lifecycle,
    ) -> Result<PublishedInterface, BindError> {
        let Some(interface) = self
            .registry
            .interfaces()
            .iter()
            .find(|interface| interface.name == name)
        else {
            return Err(BindError::NotFound);
        };

        if interface.lifecycle != lifecycle {
            return Err(BindError::WrongLifecycle);
        }
        if !can_see(interface, client) || !can_bind(interface, client) {
            return Err(BindError::PermissionDenied);
        }

        Ok(interface.clone())
    }
}

fn metadata_matches(values: &[Metadata], required: &[Metadata]) -> bool {
    required.iter().all(|filter| {
        values
            .iter()
            .any(|metadata| metadata.key == filter.key && metadata.value == filter.value)
    })
}

fn parse_metadata_filter(filter: &str) -> Result<Metadata, BindError> {
    let Some((key, value)) = filter.split_once("==") else {
        return Err(BindError::InvalidFilter);
    };
    let key = key.trim();
    let value = value.trim();
    if key.is_empty() {
        return Err(BindError::InvalidFilter);
    }
    let value = parse_quoted(value, '\'').or_else(|| parse_quoted(value, '"'));
    let Some(value) = value else {
        return Err(BindError::InvalidFilter);
    };
    Ok(Metadata {
        key: key.into(),
        value: value.into(),
    })
}

fn parse_quoted(value: &str, quote: char) -> Option<&str> {
    value.strip_prefix(quote)?.strip_suffix(quote)
}

fn scoped_capabilities<'a>(
    consumed: &ConsumedService,
    interface: &'a PublishedInterface,
) -> Result<Vec<&'a crate::manifest::CapabilityMetadata>, BindError> {
    if consumed.capabilities.is_empty() {
        return Ok(interface.capabilities.iter().collect());
    }

    let mut scoped = Vec::new();
    for requested in &consumed.capabilities {
        if requested.capability.is_empty() {
            return Err(BindError::InvalidCapability);
        }
        let Some(provider) = interface
            .capabilities
            .iter()
            .find(|capability| capability.capability == requested.capability)
        else {
            return Err(BindError::InvalidCapability);
        };
        for method in &requested.methods {
            if method.ordinal == 0 {
                return Err(BindError::InvalidCapability);
            }
            if method.link_type == LinkType::Required
                && !provider.method_ordinals.contains(&method.ordinal)
            {
                return Err(BindError::InvalidCapability);
            }
        }
        scoped.push(provider);
    }
    Ok(scoped)
}

fn scoped_ordinals(
    consumed: &ConsumedService,
    capability: &crate::manifest::CapabilityMetadata,
) -> Result<Vec<u64>, BindError> {
    let Some(requested) = consumed
        .capabilities
        .iter()
        .find(|requested| requested.capability == capability.capability)
    else {
        return Ok(Vec::new());
    };

    let mut ordinals = Vec::new();
    for method in &requested.methods {
        if method.ordinal == 0 {
            return Err(BindError::InvalidCapability);
        }
        if capability.method_ordinals.contains(&method.ordinal) {
            if !ordinals.contains(&method.ordinal) {
                ordinals.push(method.ordinal);
            }
        } else if method.link_type == LinkType::Required {
            return Err(BindError::InvalidCapability);
        }
    }
    Ok(ordinals)
}

fn can_bind(interface: &PublishedInterface, client: &ClientContext) -> bool {
    check_permission(interface.bind_permission.as_deref(), client) == PermissionDecision::Allow
}

fn can_see(interface: &PublishedInterface, client: &ClientContext) -> bool {
    match interface.visibility {
        Visibility::Public => true,
        Visibility::Private => interface.provider_package == client.package_name,
        Visibility::DomainShared => {
            package_domain(&interface.provider_package) == package_domain(&client.package_name)
        }
        Visibility::Unspecified => false,
    }
}

fn package_domain(package: &str) -> &str {
    package
        .split_once(':')
        .map_or(package, |(domain, _)| domain)
}
