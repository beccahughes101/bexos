use alloc::string::String;
use alloc::vec::Vec;
use bexos_kernel_core::ipc::Capability;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BusType {
    Pci,
    Usb,
    PlatformDt,
    I2c,
    Spi,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceProperty {
    pub key: String,
    pub value: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceNodeInfo {
    pub node_id: u64,
    pub bus: BusType,
    pub parent_node_id: Option<u64>,
    pub topological_path: String,
    pub properties: Vec<DeviceProperty>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HardwareResourceKind {
    Mmio,
    Interrupt,
    DmaPool,
    IommuDomain,
    RegisterProxy,
    BusControl,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HardwareResourceLease {
    pub kind: HardwareResourceKind,
    pub resource_id: u64,
    pub base: u64,
    pub length: u64,
    pub flags: u64,
    pub capability: Capability,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceRegistrar {
    pub package_id: String,
    pub node_id: Option<u64>,
    pub system_privileged: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegisteredDeviceNode {
    pub info: DeviceNodeInfo,
    pub resources: Vec<HardwareResourceLease>,
    pub mmio_vmo: Option<Capability>,
    pub irq_channel: Option<Capability>,
    pub registrar: Option<DeviceRegistrar>,
    pub present: bool,
    pub state: DeviceNodeState,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeviceRegistryError {
    DuplicateNode(u64),
    EmptyPropertyKey,
    DuplicateProperty(String),
    DuplicateResource(u64),
    MissingResourceHandle(u64),
    MissingParent(u64),
    InvalidParent(u64),
    AccessDenied(u64),
    NotFound(u64),
    AlreadyBinding(u64),
    AlreadyActive(u64),
    BadState(u64),
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum DeviceNodeState {
    #[default]
    Unbound,
    Binding(DriverBinding),
    Active(DriverBinding),
    Quiescing(DriverBinding),
    Suspended(DriverBinding),
    BindFailed {
        package_id: String,
        process_name: String,
    },
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DriverBinding {
    pub package_id: String,
    pub process_name: String,
    pub process_handle: Option<Capability>,
    pub manager_channel: Option<Capability>,
    pub lifecycle_channel: Option<Capability>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DeviceRegistry {
    nodes: Vec<RegisteredDeviceNode>,
}

impl DeviceRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_device_node(
        &mut self,
        node: RegisteredDeviceNode,
    ) -> Result<(), DeviceRegistryError> {
        self.register_device_node_from(None, node)
    }

    pub fn register_device_node_from(
        &mut self,
        registrar: Option<DeviceRegistrar>,
        mut node: RegisteredDeviceNode,
    ) -> Result<(), DeviceRegistryError> {
        self.validate_new_node(&node, registrar.as_ref())?;
        if node.mmio_vmo.is_none() {
            node.mmio_vmo = node
                .resources
                .iter()
                .find(|resource| resource.kind == HardwareResourceKind::Mmio)
                .map(|resource| resource.capability);
        }
        if node.irq_channel.is_none() {
            node.irq_channel = node
                .resources
                .iter()
                .find(|resource| resource.kind == HardwareResourceKind::Interrupt)
                .map(|resource| resource.capability);
        }
        node.registrar = registrar.or(node.registrar);
        node.present = true;
        node.state = DeviceNodeState::Unbound;
        self.nodes.push(node);
        Ok(())
    }

    pub fn restore_device_node(
        &mut self,
        mut node: RegisteredDeviceNode,
    ) -> Result<(), DeviceRegistryError> {
        self.validate_new_node(&node, None)?;
        if node.mmio_vmo.is_none() {
            node.mmio_vmo = node
                .resources
                .iter()
                .find(|resource| resource.kind == HardwareResourceKind::Mmio)
                .map(|resource| resource.capability);
        }
        if node.irq_channel.is_none() {
            node.irq_channel = node
                .resources
                .iter()
                .find(|resource| resource.kind == HardwareResourceKind::Interrupt)
                .map(|resource| resource.capability);
        }
        self.nodes.push(node);
        Ok(())
    }

    fn validate_new_node(
        &self,
        node: &RegisteredDeviceNode,
        registrar: Option<&DeviceRegistrar>,
    ) -> Result<(), DeviceRegistryError> {
        if self
            .nodes
            .iter()
            .any(|existing| existing.info.node_id == node.info.node_id)
        {
            return Err(DeviceRegistryError::DuplicateNode(node.info.node_id));
        }
        if node.info.parent_node_id == Some(node.info.node_id) {
            return Err(DeviceRegistryError::InvalidParent(node.info.node_id));
        }
        if node.info.topological_path.is_empty()
            || node.info.topological_path.len() > 256
            || !node.info.topological_path.is_ascii()
            || node.info.topological_path.starts_with('/')
            || node.info.topological_path.ends_with('/')
            || node
                .info
                .topological_path
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
            || self
                .nodes
                .iter()
                .any(|existing| existing.info.topological_path == node.info.topological_path)
        {
            return Err(DeviceRegistryError::InvalidParent(node.info.node_id));
        }
        if let Some(parent_id) = node.info.parent_node_id {
            let parent = self
                .nodes
                .iter()
                .find(|parent| parent.info.node_id == parent_id)
                .ok_or(DeviceRegistryError::MissingParent(parent_id))?;
            let prefix = alloc::format!("{}/", parent.info.topological_path);
            if !node.info.topological_path.starts_with(&prefix) {
                return Err(DeviceRegistryError::InvalidParent(node.info.node_id));
            }
        }
        if let Some(parent_id) = node.info.parent_node_id {
            if !self
                .nodes
                .iter()
                .any(|parent| parent.info.node_id == parent_id)
            {
                return Err(DeviceRegistryError::MissingParent(parent_id));
            }
        }
        if let Some(registrar) = registrar {
            if !registrar.system_privileged && registrar.node_id != node.info.parent_node_id {
                return Err(DeviceRegistryError::AccessDenied(node.info.node_id));
            }
        }
        if node
            .info
            .properties
            .iter()
            .any(|property| property.key.is_empty())
        {
            return Err(DeviceRegistryError::EmptyPropertyKey);
        }
        for (index, property) in node.info.properties.iter().enumerate() {
            if node.info.properties[..index]
                .iter()
                .any(|prior| prior.key == property.key)
            {
                return Err(DeviceRegistryError::DuplicateProperty(property.key.clone()));
            }
        }
        for (index, resource) in node.resources.iter().enumerate() {
            if resource.capability.object_id == 0 {
                return Err(DeviceRegistryError::MissingResourceHandle(
                    resource.resource_id,
                ));
            }
            if node.resources[..index]
                .iter()
                .any(|prior| prior.resource_id == resource.resource_id)
            {
                return Err(DeviceRegistryError::DuplicateResource(resource.resource_id));
            }
        }
        Ok(())
    }

    pub fn unregister_device_node(
        &mut self,
        node_id: u64,
    ) -> Result<RegisteredDeviceNode, DeviceRegistryError> {
        let removed = self.unregister_device_node_post_order(node_id)?;
        removed
            .into_iter()
            .last()
            .ok_or(DeviceRegistryError::NotFound(node_id))
    }

    pub fn unregister_device_node_post_order(
        &mut self,
        node_id: u64,
    ) -> Result<Vec<RegisteredDeviceNode>, DeviceRegistryError> {
        if !self.nodes.iter().any(|node| node.info.node_id == node_id) {
            return Err(DeviceRegistryError::NotFound(node_id));
        }
        let mut ids = Vec::new();
        self.collect_post_order(node_id, &mut ids);
        let mut removed = Vec::new();
        for id in ids {
            let index = self
                .nodes
                .iter()
                .position(|node| node.info.node_id == id)
                .ok_or(DeviceRegistryError::NotFound(id))?;
            removed.push(self.nodes.remove(index));
        }
        Ok(removed)
    }

    pub fn unregister_device_node_from(
        &mut self,
        registrar: &DeviceRegistrar,
        node_id: u64,
    ) -> Result<Vec<RegisteredDeviceNode>, DeviceRegistryError> {
        let node = self
            .nodes
            .iter()
            .find(|node| node.info.node_id == node_id)
            .ok_or(DeviceRegistryError::NotFound(node_id))?;
        if !registrar.system_privileged && node.registrar.as_ref() != Some(registrar) {
            return Err(DeviceRegistryError::AccessDenied(node_id));
        }
        self.unregister_device_node_post_order(node_id)
    }

    pub fn mark_removed_post_order(
        &mut self,
        node_id: u64,
    ) -> Result<Vec<RegisteredDeviceNode>, DeviceRegistryError> {
        if !self.nodes.iter().any(|node| node.info.node_id == node_id) {
            return Err(DeviceRegistryError::NotFound(node_id));
        }
        let mut ids = Vec::new();
        self.collect_post_order(node_id, &mut ids);
        let mut out = Vec::new();
        for id in ids {
            let node = self.node_mut(id)?;
            node.present = false;
            out.push(node.clone());
        }
        Ok(out)
    }

    pub fn reset_node_recovery(&mut self, node_id: u64) -> Result<(), DeviceRegistryError> {
        let index = self
            .nodes
            .iter()
            .position(|node| node.info.node_id == node_id)
            .ok_or(DeviceRegistryError::NotFound(node_id))?;
        self.nodes[index].state = DeviceNodeState::Unbound;
        Ok(())
    }

    pub fn find_by_property(&self, key: &str, value: u32) -> Vec<&RegisteredDeviceNode> {
        self.nodes
            .iter()
            .filter(|node| {
                node.info
                    .properties
                    .iter()
                    .any(|property| property.key == key && property.value == value)
            })
            .collect()
    }

    pub fn begin_binding(
        &mut self,
        node_id: u64,
        package_id: String,
        process_name: String,
    ) -> Result<(), DeviceRegistryError> {
        let node = self.node_mut(node_id)?;
        match node.state {
            DeviceNodeState::Unbound | DeviceNodeState::BindFailed { .. } => {
                node.state = DeviceNodeState::Binding(DriverBinding {
                    package_id,
                    process_name,
                    process_handle: None,
                    manager_channel: None,
                    lifecycle_channel: None,
                });
                Ok(())
            }
            DeviceNodeState::Binding(_) => Err(DeviceRegistryError::AlreadyBinding(node_id)),
            DeviceNodeState::Active(_) => Err(DeviceRegistryError::AlreadyActive(node_id)),
            DeviceNodeState::Quiescing(_) | DeviceNodeState::Suspended(_) => {
                Err(DeviceRegistryError::AlreadyActive(node_id))
            }
        }
    }

    pub fn bind_active(
        &mut self,
        node_id: u64,
        process_handle: Option<Capability>,
        manager_channel: Option<Capability>,
        lifecycle_channel: Option<Capability>,
    ) -> Result<(), DeviceRegistryError> {
        let node = self.node_mut(node_id)?;
        let DeviceNodeState::Binding(binding) = &mut node.state else {
            return Err(DeviceRegistryError::NotFound(node_id));
        };
        binding.process_handle = process_handle;
        binding.manager_channel = manager_channel;
        binding.lifecycle_channel = lifecycle_channel;
        node.state = DeviceNodeState::Active(binding.clone());
        Ok(())
    }

    /// Retarget registry references before the retired driver's process
    /// descriptor is closed. Its preserved channels keep their identities.
    pub fn replace_process_handle(&mut self, old: u64, replacement: u64) {
        for node in &mut self.nodes {
            let binding = match &mut node.state {
                DeviceNodeState::Binding(binding)
                | DeviceNodeState::Active(binding)
                | DeviceNodeState::Quiescing(binding)
                | DeviceNodeState::Suspended(binding) => binding,
                DeviceNodeState::Unbound | DeviceNodeState::BindFailed { .. } => continue,
            };
            if let Some(capability) = &mut binding.process_handle {
                if capability.object_id == old {
                    capability.object_id = replacement;
                }
            }
        }
    }

    pub fn bind_failed(
        &mut self,
        node_id: u64,
        package_id: String,
        process_name: String,
    ) -> Result<(), DeviceRegistryError> {
        let node = self.node_mut(node_id)?;
        node.state = DeviceNodeState::BindFailed {
            package_id,
            process_name,
        };
        Ok(())
    }

    pub fn begin_quiescing(&mut self, node_id: u64) -> Result<(), DeviceRegistryError> {
        let node = self.node_mut(node_id)?;
        let DeviceNodeState::Active(binding) = &node.state else {
            return Err(DeviceRegistryError::NotFound(node_id));
        };
        node.state = DeviceNodeState::Quiescing(binding.clone());
        Ok(())
    }

    pub fn suspend_active(&mut self, node_id: u64) -> Result<(), DeviceRegistryError> {
        let node = self.node_mut(node_id)?;
        let DeviceNodeState::Active(binding) = &node.state else {
            return Err(DeviceRegistryError::NotFound(node_id));
        };
        node.state = DeviceNodeState::Suspended(binding.clone());
        Ok(())
    }

    pub fn nodes(&self) -> &[RegisteredDeviceNode] {
        &self.nodes
    }

    pub fn replace_nodes(
        &mut self,
        nodes: Vec<RegisteredDeviceNode>,
    ) -> Result<(), DeviceRegistryError> {
        let mut next = Self::new();
        for node in nodes {
            next.register_device_node(node)?;
        }
        *self = next;
        Ok(())
    }

    fn collect_post_order(&self, node_id: u64, out: &mut Vec<u64>) {
        let children: Vec<u64> = self
            .nodes
            .iter()
            .filter(|node| node.info.parent_node_id == Some(node_id))
            .map(|node| node.info.node_id)
            .collect();
        for child in children {
            self.collect_post_order(child, out);
        }
        out.push(node_id);
    }

    fn node_mut(&mut self, node_id: u64) -> Result<&mut RegisteredDeviceNode, DeviceRegistryError> {
        self.nodes
            .iter_mut()
            .find(|node| node.info.node_id == node_id)
            .ok_or(DeviceRegistryError::NotFound(node_id))
    }
}
