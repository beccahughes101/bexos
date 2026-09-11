extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use bexos_kernel_core::ipc::Capability;
use bexos_userspace::{Channel, Memory};

use crate::BoundCapability;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RetainedProviderEndpoint {
    pub node_id: u64,
    pub provider_package: String,
    pub service_name: String,
    pub protocol: String,
    pub capability: String,
    pub method_ordinals: Vec<u64>,
    pub permission_values: Vec<String>,
    pub metadata: Vec<u8>,
    pub endpoint: Capability,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DriverRouteTable {
    routes: Vec<RetainedProviderEndpoint>,
}

impl DriverRouteTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn retain(&mut self, node_id: u64, binding: &BoundCapability, metadata: Vec<u8>) {
        if self.routes.iter().any(|route| {
            route.endpoint.object_id == binding.provider_endpoint.object_id
                && route.node_id == node_id
                && route.service_name == binding.service_name
        }) {
            return;
        }
        self.routes.push(RetainedProviderEndpoint {
            node_id,
            provider_package: binding.provider_package.clone(),
            service_name: binding.service_name.clone(),
            protocol: binding.protocol.clone(),
            capability: binding.capability.clone(),
            method_ordinals: binding.method_ordinals.clone(),
            permission_values: binding.permission_values.clone(),
            metadata,
            endpoint: binding.provider_endpoint,
        });
    }

    pub fn replay_node(
        &self,
        node_id: u64,
        manager: Channel,
    ) -> Result<usize, kernel_fidl::Status> {
        let mut replayed = 0;
        for route in self.routes.iter().filter(|route| route.node_id == node_id) {
            let endpoint = Memory::duplicate(route.endpoint.object_id, route.endpoint.rights)
                .map_err(|_| kernel_fidl::Status::ErrInvalidHandle)?;
            manager.send(&route.metadata, &[endpoint])?;
            replayed += 1;
        }
        Ok(replayed)
    }

    pub fn close_node(&mut self, node_id: u64) {
        let mut retained = Vec::new();
        for route in self.routes.drain(..) {
            if route.node_id == node_id {
                let _ = Memory::close(route.endpoint.object_id);
            } else {
                retained.push(route);
            }
        }
        self.routes = retained;
    }

    pub fn routes(&self) -> &[RetainedProviderEndpoint] {
        &self.routes
    }

    pub fn restore(&mut self, route: RetainedProviderEndpoint) {
        self.routes.push(route);
    }
}
