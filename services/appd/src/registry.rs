use alloc::string::String;
use alloc::vec::Vec;
use bexos_kernel_core::ipc::Capability;

use crate::manifest::{
    CapabilityMetadata, DEFAULT_LAZY_IDLE_TIMEOUT_MS, ExposedService, Lifecycle, Metadata,
    ServiceActivation, Visibility,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishedInterface {
    pub provider_package: String,
    pub instance_id: Option<String>,
    pub name: String,
    pub protocol: String,
    pub lifecycle: Lifecycle,
    pub visibility: Visibility,
    pub bind_permission: Option<String>,
    pub metadata: Vec<Metadata>,
    pub capabilities: Vec<CapabilityMetadata>,
    pub endpoint: Capability,
    pub activation: ServiceActivation,
    pub idle_timeout_ms: u32,
    pub provider_process: Option<String>,
    pub dormant: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RegistryError {
    DuplicateInterface {
        provider_package: String,
        name: String,
    },
    DuplicateSingleton {
        name: String,
        existing_provider_package: String,
        provider_package: String,
    },
    EmptyName,
    EmptyProtocol,
    InvalidLazyProvider,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CapabilityRegistry {
    interfaces: Vec<PublishedInterface>,
}

impl CapabilityRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn publish_interface(
        &mut self,
        provider_package: impl Into<String>,
        service: ExposedService,
        endpoint: Capability,
    ) -> Result<(), RegistryError> {
        if service.name.is_empty() {
            return Err(RegistryError::EmptyName);
        }
        if service.protocol.is_empty() {
            return Err(RegistryError::EmptyProtocol);
        }

        let provider_package = provider_package.into();
        if self
            .interfaces
            .iter()
            .any(|entry| entry.provider_package == provider_package && entry.name == service.name)
        {
            return Err(RegistryError::DuplicateInterface {
                provider_package,
                name: service.name,
            });
        }
        if service.lifecycle == Lifecycle::Singleton {
            if let Some(existing) = self
                .interfaces
                .iter()
                .find(|entry| entry.lifecycle == Lifecycle::Singleton && entry.name == service.name)
            {
                return Err(RegistryError::DuplicateSingleton {
                    name: service.name,
                    existing_provider_package: existing.provider_package.clone(),
                    provider_package,
                });
            }
        }

        let mut capabilities = service.capabilities;
        if capabilities.is_empty() {
            capabilities.push(CapabilityMetadata {
                capability: "Public".into(),
                permission: None,
                method_ordinals: Vec::new(),
            });
        }

        let activation = service.activation;
        let idle_timeout_ms =
            service
                .idle_timeout_ms
                .unwrap_or(if activation == ServiceActivation::Lazy {
                    DEFAULT_LAZY_IDLE_TIMEOUT_MS
                } else {
                    0
                });
        self.interfaces.push(PublishedInterface {
            provider_package,
            instance_id: None,
            name: service.name,
            protocol: service.protocol,
            lifecycle: service.lifecycle,
            visibility: service.visibility,
            bind_permission: service.bind_permission,
            metadata: service.metadata,
            capabilities,
            endpoint,
            activation,
            idle_timeout_ms,
            provider_process: service.provider_process,
            dormant: false,
        });
        Ok(())
    }

    pub fn publish_dormant_interface(
        &mut self,
        provider_package: impl Into<String>,
        mut service: ExposedService,
        provider_process: impl Into<String>,
    ) -> Result<(), RegistryError> {
        let provider_process = provider_process.into();
        if provider_process.is_empty() || service.activation != ServiceActivation::Lazy {
            return Err(RegistryError::InvalidLazyProvider);
        }
        service.provider_process = Some(provider_process);
        self.publish_interface_inner(
            provider_package.into(),
            None,
            service,
            Capability {
                object_id: 0,
                rights: 0,
            },
            true,
        )
    }

    pub fn publish_instance_interface(
        &mut self,
        provider_package: impl Into<String>,
        instance_id: impl Into<String>,
        mut service: ExposedService,
        endpoint: Capability,
    ) -> Result<(), RegistryError> {
        let instance_id = instance_id.into();
        let metadata = Metadata {
            key: "device.node_id".into(),
            value: instance_id.clone(),
        };
        if !service
            .metadata
            .iter()
            .any(|entry| entry.key == metadata.key)
        {
            service.metadata.push(metadata);
        }
        self.publish_interface_inner(
            provider_package.into(),
            Some(instance_id),
            service,
            endpoint,
            false,
        )
    }

    fn publish_interface_inner(
        &mut self,
        provider_package: String,
        instance_id: Option<String>,
        service: ExposedService,
        endpoint: Capability,
        dormant: bool,
    ) -> Result<(), RegistryError> {
        if service.name.is_empty() {
            return Err(RegistryError::EmptyName);
        }
        if service.protocol.is_empty() {
            return Err(RegistryError::EmptyProtocol);
        }

        if self.interfaces.iter().any(|entry| {
            entry.provider_package == provider_package
                && entry.name == service.name
                && entry.instance_id == instance_id
        }) {
            return Err(RegistryError::DuplicateInterface {
                provider_package,
                name: service.name,
            });
        }
        if service.lifecycle == Lifecycle::Singleton {
            if let Some(existing) = self
                .interfaces
                .iter()
                .find(|entry| entry.lifecycle == Lifecycle::Singleton && entry.name == service.name)
            {
                return Err(RegistryError::DuplicateSingleton {
                    name: service.name,
                    existing_provider_package: existing.provider_package.clone(),
                    provider_package,
                });
            }
        }

        let mut capabilities = service.capabilities;
        if capabilities.is_empty() {
            capabilities.push(CapabilityMetadata {
                capability: "Public".into(),
                permission: None,
                method_ordinals: Vec::new(),
            });
        }

        let activation = service.activation;
        let idle_timeout_ms =
            service
                .idle_timeout_ms
                .unwrap_or(if activation == ServiceActivation::Lazy {
                    DEFAULT_LAZY_IDLE_TIMEOUT_MS
                } else {
                    0
                });
        self.interfaces.push(PublishedInterface {
            provider_package,
            instance_id,
            name: service.name,
            protocol: service.protocol,
            lifecycle: service.lifecycle,
            visibility: service.visibility,
            bind_permission: service.bind_permission,
            metadata: service.metadata,
            capabilities,
            endpoint,
            activation,
            idle_timeout_ms,
            provider_process: service.provider_process,
            dormant,
        });
        Ok(())
    }

    pub fn update_provider_manager(
        &mut self,
        provider_package: &str,
        process_name: &str,
        endpoint: Capability,
    ) {
        for interface in &mut self.interfaces {
            if interface.provider_package == provider_package
                && interface.provider_process.as_deref() == Some(process_name)
            {
                interface.endpoint = endpoint;
                interface.dormant = false;
            }
        }
    }

    pub fn update_instance_manager(
        &mut self,
        provider_package: &str,
        instance_id: &str,
        endpoint: Capability,
    ) {
        for interface in &mut self.interfaces {
            if interface.provider_package == provider_package
                && interface.instance_id.as_deref() == Some(instance_id)
            {
                interface.endpoint = endpoint;
            }
        }
    }

    pub fn remove_instance(&mut self, provider_package: &str, instance_id: &str) {
        self.interfaces.retain(|interface| {
            !(interface.provider_package == provider_package
                && interface.instance_id.as_deref() == Some(instance_id))
        });
    }

    pub fn remove_provider_package(&mut self, provider_package: &str) {
        self.interfaces
            .retain(|interface| interface.provider_package != provider_package);
    }

    pub fn interfaces(&self) -> &[PublishedInterface] {
        &self.interfaces
    }

    pub fn replace_interfaces(
        &mut self,
        interfaces: Vec<PublishedInterface>,
    ) -> Result<(), RegistryError> {
        let mut next = Self::new();
        for interface in interfaces {
            next.publish_interface_inner(
                interface.provider_package,
                interface.instance_id,
                ExposedService {
                    name: interface.name,
                    protocol: interface.protocol,
                    lifecycle: interface.lifecycle,
                    visibility: interface.visibility,
                    bind_permission: interface.bind_permission,
                    metadata: interface.metadata,
                    capabilities: interface.capabilities,
                    activation: interface.activation,
                    idle_timeout_ms: Some(interface.idle_timeout_ms),
                    provider_process: interface.provider_process,
                },
                interface.endpoint,
                interface.dormant,
            )?;
        }
        *self = next;
        Ok(())
    }
}
