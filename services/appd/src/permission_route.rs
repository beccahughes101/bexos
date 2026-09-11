extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use bexos_kernel_core::ipc::Capability;

use crate::{BoundCapability, Manifest};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PermissionRoute {
    pub caller_package: String,
    pub caller_process: String,
    pub caller_uid: u64,
    pub provider_package: String,
    pub provider_instance_id: Option<String>,
    pub service_name: String,
    pub protocol: String,
    pub capability: String,
    pub permission: Option<String>,
    pub method_ordinals: Vec<u64>,
    pub granted_values: Vec<String>,
    pub metadata: Vec<u8>,
    pub retained_endpoint: Capability,
    pub client_endpoint: Capability,
    pub provider_manager: Capability,
}

impl PermissionRoute {
    pub fn from_binding(
        caller_process: &str,
        binding: &BoundCapability,
        metadata: Vec<u8>,
    ) -> Self {
        Self {
            caller_package: binding.caller_package.clone(),
            caller_process: caller_process.into(),
            caller_uid: binding.caller_uid.unwrap_or(0),
            provider_package: binding.provider_package.clone(),
            provider_instance_id: binding.provider_instance_id.clone(),
            service_name: binding.service_name.clone(),
            protocol: binding.protocol.clone(),
            capability: binding.capability.clone(),
            permission: binding.permission.clone(),
            method_ordinals: binding.method_ordinals.clone(),
            granted_values: binding.permission_values.clone(),
            metadata,
            retained_endpoint: binding.provider_endpoint,
            client_endpoint: binding.client_endpoint,
            provider_manager: binding.provider_manager,
        }
    }

    pub fn compatible_manifest(&self, manifest: &Manifest) -> bool {
        if manifest.package_name != self.provider_package {
            return false;
        }
        manifest.services_exposed.iter().any(|service| {
            service.name == self.service_name
                && service.protocol == self.protocol
                && service.capabilities.iter().any(|capability| {
                    capability.capability == self.capability
                        && capability.permission == self.permission
                        && self
                            .method_ordinals
                            .iter()
                            .all(|ordinal| capability.method_ordinals.contains(ordinal))
                })
        })
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PermissionRouteTable {
    routes: Vec<PermissionRoute>,
}

impl PermissionRouteTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn retain(&mut self, route: PermissionRoute) {
        self.routes.push(route);
    }

    pub fn routes(&self) -> &[PermissionRoute] {
        &self.routes
    }

    pub fn routes_mut(&mut self) -> &mut [PermissionRoute] {
        &mut self.routes
    }

    pub fn remove_client_endpoint(&mut self, endpoint: u64) -> Vec<PermissionRoute> {
        self.drain_matching(|route| route.client_endpoint.object_id == endpoint)
    }

    pub fn remove_caller_process(
        &mut self,
        package: &str,
        process: &str,
        uid: u64,
    ) -> Vec<PermissionRoute> {
        self.drain_matching(|route| {
            route.caller_package == package
                && route.caller_process == process
                && route.caller_uid == uid
        })
    }

    pub fn remove_caller_package(&mut self, package: &str) -> Vec<PermissionRoute> {
        self.drain_matching(|route| route.caller_package == package)
    }

    pub fn remove_provider_package(&mut self, package: &str) -> Vec<PermissionRoute> {
        self.drain_matching(|route| route.provider_package == package)
    }

    pub fn remove_uid(&mut self, uid: u64) -> Vec<PermissionRoute> {
        self.drain_matching(|route| route.caller_uid == uid)
    }

    pub fn take_incompatible_provider(
        &mut self,
        provider_package: &str,
        manifest: &Manifest,
    ) -> Vec<PermissionRoute> {
        self.drain_matching(|route| {
            route.provider_package == provider_package && !route.compatible_manifest(manifest)
        })
    }

    pub fn replayable_for_provider(
        &mut self,
        provider_package: &str,
        manager: Capability,
    ) -> Vec<usize> {
        let mut indices = Vec::new();
        for (index, route) in self.routes.iter_mut().enumerate() {
            if route.provider_package == provider_package {
                route.provider_manager = manager;
                indices.push(index);
            }
        }
        indices
    }

    fn drain_matching<F>(&mut self, mut matches: F) -> Vec<PermissionRoute>
    where
        F: FnMut(&PermissionRoute) -> bool,
    {
        let mut removed = Vec::new();
        let mut index = 0;
        while index < self.routes.len() {
            if matches(&self.routes[index]) {
                removed.push(self.routes.remove(index));
            } else {
                index += 1;
            }
        }
        removed
    }
}
