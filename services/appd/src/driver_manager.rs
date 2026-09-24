use alloc::string::String;
use alloc::vec::Vec;

use crate::device_registry::{BusType, DeviceNodeInfo, HardwareResourceKind, RegisteredDeviceNode};
use crate::manifest::{
    BindBusType, BindCondition, BindRule, DriverHardwareResourceKind, ExposedService, Manifest,
    Process,
};
use crate::platform_config::{DriverPolicy, HardwareAccessTier, PackageIdentity};
use crate::runner::hardware_access_for_driver;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DriverCandidate<'a> {
    pub manifest: &'a Manifest,
    pub process: &'a Process,
    pub hardware_access: HardwareAccessTier,
    pub score: u32,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DriverIndex<'a> {
    manifests: Vec<&'a Manifest>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DriverExclusions {
    entries: Vec<DriverExclusion>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DriverExclusion {
    pub node_id: u64,
    pub package_id: String,
    pub process_name: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ServiceContract {
    pub services: Vec<ExposedService>,
}

impl<'a> DriverIndex<'a> {
    pub fn build(manifests: &'a [Manifest]) -> Self {
        Self {
            manifests: manifests
                .iter()
                .filter(|manifest| {
                    manifest.driver_info.is_some() || !manifest.bind_rules.is_empty()
                })
                .collect(),
        }
    }

    pub fn best_match<I>(
        &self,
        node: &DeviceNodeInfo,
        driver_policy: &DriverPolicy,
        identity_for_manifest: I,
    ) -> Option<DriverCandidate<'a>>
    where
        I: FnMut(&'a Manifest) -> PackageIdentity<'a>,
    {
        self.ranked_candidates(node, driver_policy, identity_for_manifest, None, None)
            .into_iter()
            .next()
    }

    pub fn best_match_for_registered<I>(
        &self,
        node: &RegisteredDeviceNode,
        driver_policy: &DriverPolicy,
        identity_for_manifest: I,
    ) -> Option<DriverCandidate<'a>>
    where
        I: FnMut(&'a Manifest) -> PackageIdentity<'a>,
    {
        self.ranked_candidates(&node.info, driver_policy, identity_for_manifest, None, None)
            .into_iter()
            .find(|candidate| resources_satisfy(candidate.manifest, node))
    }

    pub fn ranked_candidates<I>(
        &self,
        node: &DeviceNodeInfo,
        driver_policy: &DriverPolicy,
        mut identity_for_manifest: I,
        exclusions: Option<&DriverExclusions>,
        required_contract: Option<&ServiceContract>,
    ) -> Vec<DriverCandidate<'a>>
    where
        I: FnMut(&'a Manifest) -> PackageIdentity<'a>,
    {
        let mut candidates = Vec::new();
        for manifest in &self.manifests {
            let Some(process) = manifest.processes.first() else {
                continue;
            };
            if exclusions.is_some_and(|excluded| {
                excluded.contains(node.node_id, &manifest.package_name, &process.name)
            }) {
                continue;
            }
            if required_contract
                .is_some_and(|contract| !contract.compatible_with(&manifest.services_exposed))
            {
                continue;
            }
            let Some(score) = manifest
                .bind_rules
                .iter()
                .filter_map(|rule| score_rule(rule, node))
                .max()
            else {
                continue;
            };
            let identity = identity_for_manifest(manifest);
            let hardware_access =
                match hardware_access_for_driver(driver_policy.evaluate_driver(identity)) {
                    Ok(hardware_access) => hardware_access,
                    Err(_) => continue,
                };
            let candidate = DriverCandidate {
                manifest,
                process,
                hardware_access,
                score,
            };
            candidates.push(candidate);
        }
        candidates.sort_by(|a, b| compare_candidates(*a, *b));
        candidates
    }

    pub fn len(&self) -> usize {
        self.manifests.len()
    }

    pub fn is_empty(&self) -> bool {
        self.manifests.is_empty()
    }
}

fn resources_satisfy(manifest: &Manifest, node: &RegisteredDeviceNode) -> bool {
    manifest.driver_info.as_ref().is_none_or(|info| {
        info.required_resources.iter().all(|requirement| {
            let kind = match requirement.kind {
                DriverHardwareResourceKind::Mmio => HardwareResourceKind::Mmio,
                DriverHardwareResourceKind::Interrupt => HardwareResourceKind::Interrupt,
                DriverHardwareResourceKind::DmaPool => HardwareResourceKind::DmaPool,
                DriverHardwareResourceKind::IommuDomain => HardwareResourceKind::IommuDomain,
                DriverHardwareResourceKind::RegisterProxy => HardwareResourceKind::RegisterProxy,
                DriverHardwareResourceKind::BusControl => HardwareResourceKind::BusControl,
                DriverHardwareResourceKind::Unspecified => return false,
            };
            node.resources
                .iter()
                .filter(|resource| resource.kind == kind)
                .count()
                >= requirement.min_count as usize
        })
    })
}

impl DriverExclusions {
    pub fn exclude(
        &mut self,
        node_id: u64,
        package_id: impl Into<String>,
        process_name: impl Into<String>,
    ) {
        let entry = DriverExclusion {
            node_id,
            package_id: package_id.into(),
            process_name: process_name.into(),
        };
        if !self.entries.contains(&entry) {
            self.entries.push(entry);
        }
    }

    pub fn reset_node(&mut self, node_id: u64) {
        self.entries.retain(|entry| entry.node_id != node_id);
    }

    pub fn contains(&self, node_id: u64, package_id: &str, process_name: &str) -> bool {
        self.entries.iter().any(|entry| {
            entry.node_id == node_id
                && entry.package_id == package_id
                && entry.process_name == process_name
        })
    }
}

impl ServiceContract {
    pub fn from_services(services: &[ExposedService]) -> Self {
        Self {
            services: services.to_vec(),
        }
    }

    pub fn compatible_with(&self, services: &[ExposedService]) -> bool {
        self.services.iter().all(|required| {
            services.iter().any(|candidate| {
                candidate.name == required.name
                    && candidate.protocol == required.protocol
                    && candidate.lifecycle == required.lifecycle
                    && required.capabilities.iter().all(|required_capability| {
                        candidate.capabilities.iter().any(|candidate_capability| {
                            candidate_capability.capability == required_capability.capability
                                && required_capability.method_ordinals.iter().all(|ordinal| {
                                    candidate_capability.method_ordinals.contains(ordinal)
                                })
                        })
                    })
            })
        })
    }
}

pub fn driver_package_id(manifest: &Manifest) -> &str {
    manifest
        .driver_info
        .as_ref()
        .and_then(|info| (!info.package_id.is_empty()).then_some(info.package_id.as_str()))
        .unwrap_or(manifest.package_name.as_str())
}

fn compare_candidates(a: DriverCandidate<'_>, b: DriverCandidate<'_>) -> core::cmp::Ordering {
    b.score
        .cmp(&a.score)
        .then_with(|| a.manifest.package_name.cmp(&b.manifest.package_name))
}

fn score_rule(rule: &BindRule, node: &DeviceNodeInfo) -> Option<u32> {
    rule.conditions
        .iter()
        .filter_map(|condition| score_condition(condition, node).map(|score| score + rule.priority))
        .max()
}

fn score_condition(condition: &BindCondition, node: &DeviceNodeInfo) -> Option<u32> {
    if !bus_matches(condition.bus, node.bus) {
        return None;
    }
    if condition.properties.is_empty() {
        return None;
    }

    let mut score = 0;
    for property in &condition.properties {
        let node_value = node
            .properties
            .iter()
            .find(|node_property| node_property.key == property.key)
            .map(|node_property| node_property.value)?;
        if node_value != property.value {
            return None;
        }
        score += property_score(&property.key);
    }
    Some(score)
}

fn property_score(key: &str) -> u32 {
    match key {
        "pci.vendor_id" | "pci.device_id" => 100,
        "pci.class" | "pci.subclass" | "pci.prog_if" => 10,
        _ => 1,
    }
}

fn bus_matches(condition: BindBusType, node: BusType) -> bool {
    matches!(
        (condition, node),
        (BindBusType::Pci, BusType::Pci)
            | (BindBusType::Usb, BusType::Usb)
            | (BindBusType::PlatformDt, BusType::PlatformDt)
            | (BindBusType::I2c, BusType::I2c)
            | (BindBusType::Spi, BusType::Spi)
    )
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DriverLifecycleLog {
    events: Vec<String>,
}

impl DriverLifecycleLog {
    pub fn record(&mut self, event: String) {
        self.events.push(event);
    }

    pub fn events(&self) -> &[String] {
        &self.events
    }
}
