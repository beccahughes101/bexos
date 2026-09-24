mod launch;
mod persistence;
use crate::{
    AppdBroker, AppdSnapshot, HandlerId, HandlerRegistration, MemoryOpenerRegistry,
    MemoryPermissionStore, OpenerBinding, OpenerScope, PlatformConfig, register_manifest_openers,
};
use alloc::{
    collections::BTreeMap,
    string::{String, ToString},
    vec::Vec,
};
use bexos_app_registry::ActivePinRecord;
use bexos_app_registry::{AppRecord, MemoryAppRegistry};
use bexos_bexfs::sys_state::Slot;
use bexos_domain_association::MemoryDomainAssociationCache;
use bexos_kernel_core::ipc::Capability;
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_permission_store::{SystemGrantRecord, UserGrantRecord, UserGrantState};
use bexos_userspace::{
    Channel,
    live_migration::{Resource, State},
};
pub use launch::LaunchRecord;

pub const APPD_PACKAGE: &str = "bexos.platform.appd";
pub const REGISTRY_BASE: u64 = 100;
pub const ACTIVE_PIN_BASE: u64 = 5_000;
pub const WATCHDOG_BASE: u64 = 7_000;
pub const SERVICE_BASE: u64 = 10_000;
pub const LAUNCH_BASE: u64 = 20_000;
pub const OPENER_BINDING_BASE: u64 = 30_000;
pub const VERSION_MANAGER_BINDING_BASE: u64 = 40_000;
pub const APP_MANAGER_BINDING_BASE: u64 = 50_000;
pub const WORKER_LAUNCHER_BINDING_BASE: u64 = 60_000;
pub const WORKER_POLICY_WATCHER_BASE: u64 = 70_000;
pub const COMPONENT_CONFIG_BASE: u64 = 80_000;
pub const ROUTE_BASE: u64 = 90_000;
pub const DRIVER_LIFECYCLE_STATE_KEY: u64 = 7;
pub const PERMISSION_STATE_KEY: u64 = 8;
pub const PERMISSION_ROUTE_STATE_KEY: u64 = 9;
pub const COMMAND_STATE_KEY: u64 = 10;
pub const SERVICE_DIRECTORY_STATE_KEY: u64 = 12;
pub const LAZY_STATE_KEY: u64 = 13;
const MAX_MANAGED_RUNTIME_ENTRIES: usize = 48;
const MAX_COMPONENT_CONFIG_RECORDS: usize = 256;
const DEVICE_RECORD_MAGIC_V2: u64 = 0x4452_5632;
const DEVICE_RECORD_MAGIC_V3: u64 = 0x4452_5633;

#[derive(Clone)]
pub struct ManagedService {
    pub package: String,
    pub process: String,
    pub instance_id: String,
    pub process_handle: u64,
    pub space_handle: u64,
    pub thread_handle: u64,
    pub manager: u64,
    pub migration: u64,
    pub hardware: u32,
    pub generation: u64,
    pub archive: u64,
    pub archive_len: u64,
    pub resource_group_id: u32,
    pub resource_job: u64,
}
impl ManagedService {
    fn encode(&self) -> Vec<u8> {
        let mut w = Encoder::new();
        w.text(&self.package);
        w.text(&self.process);
        for n in [
            self.process_handle,
            self.space_handle,
            self.thread_handle,
            self.manager,
            self.migration,
            self.hardware as u64,
            self.generation,
            self.archive,
            self.archive_len,
        ] {
            w.word(n);
        }
        w.text(&self.instance_id);
        w.word(self.resource_group_id as u64);
        w.word(self.resource_job);
        w.finish()
    }
    fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = Decoder::new(bytes);
        let mut s = Self {
            package: r.text(128)?.to_string(),
            process: r.text(64)?.to_string(),
            process_handle: r.word()?,
            space_handle: r.word()?,
            thread_handle: r.word()?,
            manager: r.word()?,
            migration: r.word()?,
            hardware: u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?,
            generation: r.word()?,
            archive: r.word()?,
            archive_len: r.word()?,
            instance_id: String::new(),
            resource_group_id: 1,
            resource_job: 0,
        };
        if let Ok(instance_id) = r.text(128) {
            s.instance_id = instance_id.to_string();
        }
        if let Ok(resource_group_id) = r.word() {
            s.resource_group_id =
                u32::try_from(resource_group_id).map_err(|_| Error::InvalidData)?;
            s.resource_job = r.word()?;
        }
        r.finish()?;
        if s.hardware > 2 || s.migration == 0 {
            return Err(Error::InvalidData);
        }
        Ok(s)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComponentConfigRecord {
    pub package: String,
    pub generation: u64,
    pub bytes: Vec<u8>,
}

impl ComponentConfigRecord {
    fn encode(&self) -> Vec<u8> {
        let mut w = Encoder::new();
        w.text(&self.package);
        w.word(self.generation);
        w.bytes(&self.bytes);
        w.finish()
    }

    fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = Decoder::new(bytes);
        let record = Self {
            package: r.text(128)?.to_string(),
            generation: r.word()?,
            bytes: r.bytes(64 * 1024)?.to_vec(),
        };
        r.finish()?;
        if record.package.is_empty() {
            return Err(Error::InvalidData);
        }
        Ok(record)
    }
}

fn encode_permission_state(permissions: &MemoryPermissionStore) -> Vec<u8> {
    let mut w = Encoder::new();
    w.word(permissions.system_grants().len() as u64);
    for record in permissions.system_grants() {
        w.text(&record.package_id);
        w.text(&record.permission_name);
        w.word(record.allowed_values.len() as u64);
        for value in &record.allowed_values {
            w.text(value);
        }
        w.word(record.verified_at);
    }
    w.word(permissions.all_user_records().len() as u64);
    for record in permissions.all_user_records() {
        w.word(record.uid);
        w.text(&record.package_id);
        w.text(&record.permission_name);
        w.word(match record.state {
            UserGrantState::Granted => 1,
            UserGrantState::Denied => 2,
        });
        w.word(record.granted_values.len() as u64);
        for value in &record.granted_values {
            w.text(value);
        }
    }
    w.finish()
}

fn decode_permission_state(bytes: &[u8]) -> Result<MemoryPermissionStore, Error> {
    let mut r = Decoder::new(bytes);
    let mut system = Vec::new();
    for _ in 0..r.count(1024)? {
        let package_id = r.text(128)?.to_string();
        let permission_name = r.text(128)?.to_string();
        let mut allowed_values = Vec::new();
        for _ in 0..r.count(64)? {
            allowed_values.push(r.text(128)?.to_string());
        }
        system.push(SystemGrantRecord {
            package_id,
            permission_name,
            allowed_values,
            verified_at: r.word()?,
        });
    }
    let mut permissions = MemoryPermissionStore::new();
    permissions
        .replace_system_grants(system)
        .map_err(|_| Error::InvalidData)?;
    for _ in 0..r.count(4096)? {
        let record = UserGrantRecord {
            uid: r.word()?,
            package_id: r.text(128)?.to_string(),
            permission_name: r.text(128)?.to_string(),
            state: match r.word()? {
                1 => UserGrantState::Granted,
                2 => UserGrantState::Denied,
                _ => return Err(Error::InvalidData),
            },
            granted_values: {
                let mut values = Vec::new();
                for _ in 0..r.count(64)? {
                    values.push(r.text(128)?.to_string());
                }
                values
            },
        };
        permissions
            .replace_user_records(record.uid, {
                let mut records = permissions.user_records(record.uid);
                records.retain(|existing| {
                    existing.package_id != record.package_id
                        || existing.permission_name != record.permission_name
                });
                records.push(record);
                records
            })
            .map_err(|_| Error::InvalidData)?;
    }
    r.finish()?;
    Ok(permissions)
}

fn encode_permission_routes(routes: &crate::PermissionRouteTable) -> Vec<u8> {
    let mut w = Encoder::new();
    w.word(routes.routes().len() as u64);
    for route in routes.routes() {
        w.text(&route.caller_package);
        w.text(&route.caller_process);
        w.word(route.caller_uid);
        w.text(&route.provider_package);
        encode_optional_text(&mut w, route.provider_instance_id.as_deref());
        w.text(&route.service_name);
        w.text(&route.protocol);
        w.text(&route.capability);
        encode_optional_text(&mut w, route.permission.as_deref());
        w.word(route.method_ordinals.len() as u64);
        for ordinal in &route.method_ordinals {
            w.word(*ordinal);
        }
        w.word(route.granted_values.len() as u64);
        for value in &route.granted_values {
            w.text(value);
        }
        w.bytes(&route.metadata);
        encode_route_capability(&mut w, route.retained_endpoint);
        encode_route_capability(&mut w, route.client_endpoint);
        encode_route_capability(&mut w, route.provider_manager);
    }
    w.finish()
}

fn decode_permission_routes(bytes: &[u8]) -> Result<crate::PermissionRouteTable, Error> {
    let mut r = Decoder::new(bytes);
    let mut table = crate::PermissionRouteTable::new();
    for _ in 0..r.count(1024)? {
        let caller_package = r.text(128)?.to_string();
        let caller_process = r.text(64)?.to_string();
        let caller_uid = r.word()?;
        let provider_package = r.text(128)?.to_string();
        let provider_instance_id = decode_optional_text(&mut r, 128)?;
        let service_name = r.text(128)?.to_string();
        let protocol = r.text(128)?.to_string();
        let capability = r.text(128)?.to_string();
        let permission = decode_optional_text(&mut r, 128)?;
        let mut method_ordinals = Vec::new();
        for _ in 0..r.count(64)? {
            method_ordinals.push(r.word()?);
        }
        let mut granted_values = Vec::new();
        for _ in 0..r.count(64)? {
            granted_values.push(r.text(128)?.to_string());
        }
        let metadata = r.bytes(4096)?.to_vec();
        let retained_endpoint = decode_route_capability(&mut r)?;
        let client_endpoint = decode_route_capability(&mut r)?;
        let provider_manager = decode_route_capability(&mut r)?;
        table.retain(crate::PermissionRoute {
            caller_package,
            caller_process,
            caller_uid,
            provider_package,
            provider_instance_id,
            service_name,
            protocol,
            capability,
            permission,
            method_ordinals,
            granted_values,
            metadata,
            retained_endpoint,
            client_endpoint,
            provider_manager,
        });
    }
    r.finish()?;
    Ok(table)
}

fn encode_optional_text(w: &mut Encoder, value: Option<&str>) {
    match value {
        Some(value) => {
            w.word(1);
            w.text(value);
        }
        None => {
            w.word(0);
        }
    }
}

fn decode_optional_text(r: &mut Decoder<'_>, max_len: usize) -> Result<Option<String>, Error> {
    Ok(if r.flag()? {
        Some(r.text(max_len)?.to_string())
    } else {
        None
    })
}

fn encode_route_capability(w: &mut Encoder, capability: Capability) {
    w.word(capability.object_id);
    w.word(capability.rights as u64);
}

fn decode_route_capability(r: &mut Decoder<'_>) -> Result<Capability, Error> {
    Ok(Capability {
        object_id: r.word()?,
        rights: u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?,
    })
}

pub struct AppdState {
    pub shell: crate::shell::Session,
    pub registry: MemoryAppRegistry,
    pub config: PlatformConfig,
    pub config_bytes: Vec<u8>,
    pub broker: AppdBroker,
    pub permissions: MemoryPermissionStore,
    pub openers: MemoryOpenerRegistry,
    pub opener_bindings: Vec<OpenerBinding>,
    pub commands: crate::command_state::CommandState,
    pub version_manager_bindings: Vec<u64>,
    pub app_manager_bindings: Vec<crate::AppManagerBinding>,
    pub package_retry_after: u64,
    pub package_installs: Vec<crate::package_install::PendingInstall>,
    pub worker_launcher_bindings: Vec<crate::AppManagerBinding>,
    pub service_directory_bindings: Vec<crate::ServiceDirectoryBinding>,
    pub worker_policy_watchers: Vec<u64>,
    pub lazy: crate::lazy::LazyActivationState,
    pub domain_associations: MemoryDomainAssociationCache,
    pub devices: crate::DeviceRegistry,
    pub input_hotplug: super::input_hotplug::Hotplug,
    pub driver_routes: crate::DriverRouteTable,
    pub permission_routes: crate::PermissionRouteTable,
    pub driver_recovery: crate::DriverRecoveryBudget,
    pub driver_exclusions: crate::DriverExclusions,
    pub driver_images: crate::DriverRecoveryImageCache,
    pub vfsd: Channel,
    pub sys_state_root: Channel,
    pub active_slot: Slot,
    pub lifecycle: Channel,
    pub updated_lifecycle: Channel,
    pub users: Channel,
    pub user_watcher: Channel,
    pub migration: Channel,
    pub services: Vec<ManagedService>,
    pub launches: Vec<LaunchRecord>,
    pub watchdogs: Vec<crate::watchdog::WatchdogRecord>,
    pub generation: u64,
    pub pending_record: Option<AppRecord>,
    pub pending_archive: (u64, u64),
    pub pending_replacement: (u64, u64, u64),
    pub component_configs: Vec<ComponentConfigRecord>,
    // Rebuildable registration cache; authoritative state lives in prefsd.
    pub preference_registered: alloc::collections::BTreeSet<String>,
    pub preference_provider: (u64, u64),
    pub preferences_frozen: Option<bool>,
    pub kernel_generation_floor: u64,
    pub tee_generation_floor: u64,
    #[cfg(feature = "persistent")]
    pub stores: Option<crate::stores::AppdStores>,
    registry_count: usize,
    service_count: usize,
    active_pin_count: usize,
    watchdog_count: usize,
    incoming: BTreeMap<u64, AppRecord>,
    incoming_pins: BTreeMap<u64, ActivePinRecord>,
}
impl AppdState {
    pub fn cold(
        registry: MemoryAppRegistry,
        config: PlatformConfig,
        config_bytes: Vec<u8>,
        broker: AppdBroker,
        permissions: MemoryPermissionStore,
        openers: MemoryOpenerRegistry,
        devices: crate::DeviceRegistry,
        vfsd: Channel,
        sys_state_root: Channel,
        active_slot: Slot,
        lifecycle: Channel,
        users: Channel,
        migration: Channel,
        services: Vec<ManagedService>,
    ) -> Self {
        Self {
            shell: Default::default(),
            registry,
            config,
            config_bytes,
            broker,
            permissions,
            openers,
            opener_bindings: Vec::new(),
            commands: Default::default(),
            version_manager_bindings: Vec::new(),
            app_manager_bindings: Vec::new(),
            package_retry_after: 0,
            package_installs: Vec::new(),
            worker_launcher_bindings: Vec::new(),
            service_directory_bindings: Vec::new(),
            worker_policy_watchers: Vec::new(),
            lazy: crate::lazy::LazyActivationState::default(),
            domain_associations: MemoryDomainAssociationCache::new(),
            devices,
            input_hotplug: Default::default(),
            driver_routes: crate::DriverRouteTable::new(),
            permission_routes: crate::PermissionRouteTable::new(),
            driver_recovery: crate::DriverRecoveryBudget::new(),
            driver_exclusions: crate::DriverExclusions::default(),
            driver_images: crate::DriverRecoveryImageCache::new(),
            vfsd,
            sys_state_root,
            active_slot,
            lifecycle,
            updated_lifecycle: Channel(0),
            users,
            user_watcher: Channel(0),
            migration,
            services,
            launches: Vec::new(),
            watchdogs: Vec::new(),
            generation: 0,
            pending_record: None,
            pending_archive: (0, 0),
            pending_replacement: (0, 0, 0),
            component_configs: Vec::new(),
            preference_registered: Default::default(),
            preference_provider: (0, 0),
            preferences_frozen: None,
            kernel_generation_floor: 0,
            tee_generation_floor: 0,
            #[cfg(feature = "persistent")]
            stores: None,
            registry_count: 0,
            service_count: 0,
            active_pin_count: 0,
            watchdog_count: 0,
            incoming: BTreeMap::new(),
            incoming_pins: BTreeMap::new(),
        }
    }
    pub fn activate_record(&mut self) -> Result<(), Error> {
        #[cfg(feature = "persistent")]
        if let Some(record) = self.pending_record.as_ref() {
            if self.stores.is_none() && record.protected {
                return Err(Error::InvalidData);
            }
        }
        if let Some(record) = self.pending_record.take() {
            let replacing_appd = record.package_id == APPD_PACKAGE;
            if self.pending_archive.0 != 0 {
                if let Some(service) = self
                    .services
                    .iter_mut()
                    .find(|s| s.package == record.package_id)
                {
                    if service.archive != 0 {
                        let _ = bexos_userspace::Memory::close(service.archive);
                    }
                    (service.archive, service.archive_len) = self.pending_archive;
                }
            }
            self.pending_archive = (0, 0);
            self.registry.replace_checkpoint_record(record)?;
            if !replacing_appd {
                self.pending_replacement = (0, 0, 0);
            }
        }
        Ok(())
    }
    pub fn registry_keys(&self, old_count: usize) -> Vec<u64> {
        core::iter::once(0)
            .chain(
                (0..old_count.max(self.registry.list_packages().len()))
                    .map(|i| REGISTRY_BASE + i as u64),
            )
            .collect()
    }

    #[cfg(feature = "persistent")]
    pub fn sync_stores(&self) -> Result<(), crate::stores::StoreError> {
        // Each redb commit otherwise rewrites the complete BexFS namespace.
        // Batch this one appd checkpoint, then durably flush both backing
        // volumes before reporting completion.
        let deferral = bexos_redb::bexos_fs::defer_file_syncs();
        if let Some(stores) = &self.stores {
            stores.replace_from_memory(&self.registry, &self.openers, &self.domain_associations)?;
        }
        if self.vfsd.0 != 0 {
            crate::permission_persistence::sync_system(self.vfsd, &self.permissions)
                .map_err(|_| crate::stores::StoreError::Permissions)?;
        }
        drop(deferral);
        if self.sys_state_root.0 != 0 {
            bexos_userspace::fs::sync(self.sys_state_root)
                .map_err(|_| crate::stores::StoreError::ActivePins)?;
        }
        if self.vfsd.0 != 0 {
            let data = bexos_userspace::vfs::get_system_data_directory(self.vfsd, APPD_PACKAGE)
                .map_err(|_| crate::stores::StoreError::Permissions)?;
            let result = bexos_userspace::fs::sync(data);
            let _ = bexos_userspace::Memory::close(data.0);
            result.map_err(|_| crate::stores::StoreError::Permissions)?;
        }
        Ok(())
    }

    #[cfg(not(feature = "persistent"))]
    pub fn sync_stores(&self) -> Result<(), crate::stores::StoreError> {
        Ok(())
    }

    pub fn sync_active_pins(&self) -> Result<(), crate::stores::StoreError> {
        #[cfg(feature = "persistent")]
        if let Some(stores) = &self.stores {
            let deferral = bexos_redb::bexos_fs::defer_file_syncs();
            stores.active_pins.replace_from_memory(&self.registry)?;
            drop(deferral);
            bexos_userspace::fs::sync(self.sys_state_root)
                .map_err(|_| crate::stores::StoreError::ActivePins)?;
        }
        Ok(())
    }

    pub fn floor(&self, target: &str) -> Option<u64> {
        match target {
            "kernel" => Some(self.kernel_generation_floor),
            "tee" => Some(self.tee_generation_floor),
            _ => self
                .registry
                .record(target)
                .ok()
                .map(|record| record.accepted_generation),
        }
    }

    pub fn commit_floor(&mut self, target: &str, generation: u64) -> Result<u64, Error> {
        let value = match target {
            "kernel" => {
                self.kernel_generation_floor = self.kernel_generation_floor.max(generation);
                self.kernel_generation_floor
            }
            "tee" => {
                self.tee_generation_floor = self.tee_generation_floor.max(generation);
                self.tee_generation_floor
            }
            _ => {
                let mut record = self
                    .registry
                    .record(target)
                    .map_err(|_| Error::InvalidData)?
                    .clone();
                record.accepted_generation = record.accepted_generation.max(generation);
                self.registry.replace_checkpoint_record(record)?;
                self.registry
                    .record(target)
                    .map_err(|_| Error::InvalidData)?
                    .accepted_generation
            }
        };
        #[cfg(feature = "persistent")]
        if let Some(stores) = &self.stores {
            stores
                .generation_floors
                .commit_monotonic(target, value)
                .map_err(|_| Error::InvalidData)?;
        }
        Ok(value)
    }
}

fn encode_device_state(w: &mut Encoder, state: &crate::DeviceNodeState) {
    match state {
        crate::DeviceNodeState::Unbound => w.word(0),
        crate::DeviceNodeState::Binding(binding) => {
            w.word(1);
            encode_binding(w, binding);
        }
        crate::DeviceNodeState::Active(binding) => {
            w.word(2);
            encode_binding(w, binding);
        }
        crate::DeviceNodeState::Quiescing(binding) => {
            w.word(3);
            encode_binding(w, binding);
        }
        crate::DeviceNodeState::Suspended(binding) => {
            w.word(4);
            encode_binding(w, binding);
        }
        crate::DeviceNodeState::BindFailed {
            package_id,
            process_name,
        } => {
            w.word(5);
            w.text(package_id);
            w.text(process_name);
        }
    }
}

fn encode_binding(w: &mut Encoder, binding: &crate::DriverBinding) {
    w.text(&binding.package_id);
    w.text(&binding.process_name);
    for cap in [
        binding.process_handle,
        binding.manager_channel,
        binding.lifecycle_channel,
    ] {
        w.word(cap.is_some() as u64);
        if let Some(cap) = cap {
            w.word(cap.object_id);
            w.word(cap.rights as u64);
        }
    }
}

fn decode_device_state(r: &mut Decoder<'_>) -> Result<crate::DeviceNodeState, Error> {
    Ok(match r.word()? {
        0 => crate::DeviceNodeState::Unbound,
        1 => crate::DeviceNodeState::Binding(decode_binding(r)?),
        2 => crate::DeviceNodeState::Active(decode_binding(r)?),
        3 => crate::DeviceNodeState::Quiescing(decode_binding(r)?),
        4 => crate::DeviceNodeState::Suspended(decode_binding(r)?),
        5 => crate::DeviceNodeState::BindFailed {
            package_id: r.text(128)?.to_string(),
            process_name: r.text(64)?.to_string(),
        },
        _ => return Err(Error::InvalidData),
    })
}

fn decode_binding(r: &mut Decoder<'_>) -> Result<crate::DriverBinding, Error> {
    Ok(crate::DriverBinding {
        package_id: r.text(128)?.to_string(),
        process_name: r.text(64)?.to_string(),
        process_handle: decode_capability(r)?,
        manager_channel: decode_capability(r)?,
        lifecycle_channel: decode_capability(r)?,
    })
}

fn decode_capability(
    r: &mut Decoder<'_>,
) -> Result<Option<bexos_kernel_core::ipc::Capability>, Error> {
    Ok(if r.flag()? {
        Some(bexos_kernel_core::ipc::Capability {
            object_id: r.word()?,
            rights: u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?,
        })
    } else {
        None
    })
}

fn encode_driver_lifecycle_state(
    routes: &crate::DriverRouteTable,
    recovery: &crate::DriverRecoveryBudget,
    images: &crate::DriverRecoveryImageCache,
) -> Vec<u8> {
    let mut w = Encoder::new();
    w.word(2);
    w.word(routes.routes().len() as u64);
    for route in routes.routes() {
        w.word(route.node_id);
        w.text(&route.provider_package);
        w.text(&route.service_name);
        w.text(&route.protocol);
        w.text(&route.capability);
        w.word(route.method_ordinals.len() as u64);
        for ordinal in &route.method_ordinals {
            w.word(*ordinal);
        }
        w.word(route.permission_values.len() as u64);
        for value in &route.permission_values {
            w.text(value);
        }
        w.bytes(&route.metadata);
        w.word(route.endpoint.object_id);
        w.word(route.endpoint.rights as u64);
    }
    w.word(recovery.counters().len() as u64);
    for counter in recovery.counters() {
        w.word(counter.node_id);
        w.text(&counter.package_id);
        w.text(&counter.process_name);
        w.word(counter.first_failure_ms);
        w.word(counter.attempts as u64);
    }
    w.word(images.images().len() as u64);
    for image in images.images() {
        w.text(&image.package_id);
        w.text(&image.process_name);
        w.word(image.executable.handle);
        w.word(image.executable.len);
        w.word(image.libraries.len() as u64);
        for library in &image.libraries {
            w.text(&library.package_id);
            w.text(&library.export_name);
            w.text(&library.soname);
            w.text(&library.symbol_prefix);
            w.word(library.abi_version as u64);
            w.word(match library.kind {
                crate::PackageLibraryKind::Native => 0,
                crate::PackageLibraryKind::WasmComponent => 1,
            });
            w.word(library.direct_dependencies.len() as u64);
            for dependency in &library.direct_dependencies {
                w.text(&dependency.package_name);
                w.word(dependency.abi_version as u64);
                w.text(&dependency.soname);
            }
            w.word(library.image.handle);
            w.word(library.image.len);
        }
    }
    w.finish()
}

fn decode_driver_lifecycle_state(
    bytes: &[u8],
) -> Result<
    (
        crate::DriverRouteTable,
        crate::DriverRecoveryBudget,
        crate::DriverRecoveryImageCache,
    ),
    Error,
> {
    let mut r = Decoder::new(bytes);
    let version = r.word()?;
    if !(1..=2).contains(&version) {
        return Err(Error::UnsupportedVersion);
    }
    let mut routes = crate::DriverRouteTable::new();
    for _ in 0..r.count(1024)? {
        let route = crate::RetainedProviderEndpoint {
            node_id: r.word()?,
            provider_package: r.text(128)?.to_string(),
            service_name: r.text(128)?.to_string(),
            protocol: r.text(128)?.to_string(),
            capability: r.text(128)?.to_string(),
            method_ordinals: {
                let mut out = Vec::new();
                for _ in 0..r.count(64)? {
                    out.push(r.word()?);
                }
                out
            },
            permission_values: {
                let mut out = Vec::new();
                for _ in 0..r.count(64)? {
                    out.push(r.text(128)?.to_string());
                }
                out
            },
            metadata: r.bytes(4096)?.to_vec(),
            endpoint: bexos_kernel_core::ipc::Capability {
                object_id: r.word()?,
                rights: u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?,
            },
        };
        routes.restore(route);
    }
    let mut counters = Vec::new();
    for _ in 0..r.count(1024)? {
        counters.push(crate::NodeRecoveryCounter {
            node_id: r.word()?,
            package_id: r.text(128)?.to_string(),
            process_name: r.text(64)?.to_string(),
            first_failure_ms: r.word()?,
            attempts: u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?,
        });
    }
    let mut images = crate::DriverRecoveryImageCache::new();
    for _ in 0..r.count(1024)? {
        let package_id = r.text(128)?.to_string();
        let process_name = r.text(64)?.to_string();
        let executable = crate::CachedImageVmo {
            handle: r.word()?,
            len: r.word()?,
        };
        let mut libraries = Vec::new();
        for _ in 0..r.count(64)? {
            let package_id = r.text(128)?.to_string();
            let export_name = r.text(128)?.to_string();
            let soname = r.text(128)?.to_string();
            let (symbol_prefix, abi_version, kind, direct_dependencies) = if version >= 2 {
                let symbol_prefix = r.text(128)?.to_string();
                let abi_version = u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
                let kind = match r.word()? {
                    0 => crate::PackageLibraryKind::Native,
                    1 => crate::PackageLibraryKind::WasmComponent,
                    _ => return Err(Error::InvalidData),
                };
                let mut direct_dependencies = Vec::new();
                for _ in 0..r.count(64)? {
                    direct_dependencies.push(crate::PackageLibraryDependency {
                        package_name: r.text(128)?.to_string(),
                        abi_version: u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?,
                        soname: r.text(128)?.to_string(),
                    });
                }
                (symbol_prefix, abi_version, kind, direct_dependencies)
            } else {
                (
                    String::new(),
                    0,
                    crate::PackageLibraryKind::Native,
                    Vec::new(),
                )
            };
            libraries.push(crate::CachedLibraryVmo {
                package_id,
                export_name,
                soname,
                symbol_prefix,
                abi_version,
                kind,
                direct_dependencies,
                image: crate::CachedImageVmo {
                    handle: r.word()?,
                    len: r.word()?,
                },
            });
        }
        images.upsert(crate::DriverRecoveryImage {
            package_id,
            process_name,
            executable,
            libraries,
        });
    }
    r.finish()?;
    Ok((
        routes,
        crate::DriverRecoveryBudget::from_counters(counters),
        images,
    ))
}

impl State for AppdState {
    fn empty() -> Self {
        Self::cold(
            MemoryAppRegistry::new(),
            PlatformConfig::default(),
            Vec::new(),
            AppdBroker::new(),
            MemoryPermissionStore::new(),
            MemoryOpenerRegistry::new(),
            crate::DeviceRegistry::new(),
            Channel(0),
            Channel(0),
            Slot::A,
            Channel(0),
            Channel(0),
            Channel(0),
            Vec::new(),
        )
    }
    fn keys(&self) -> Vec<u64> {
        (0..5)
            .filter(|k| *k != 4 || self.pending_record.is_some())
            .chain([
                5,
                6,
                crate::package_install::KEY,
                DRIVER_LIFECYCLE_STATE_KEY,
                PERMISSION_STATE_KEY,
                PERMISSION_ROUTE_STATE_KEY,
                COMMAND_STATE_KEY,
                super::shell::KEY,
            ])
            .chain(
                (!self.service_directory_bindings.is_empty())
                    .then_some(SERVICE_DIRECTORY_STATE_KEY),
            )
            .chain((!self.lazy.providers().is_empty()).then_some(LAZY_STATE_KEY))
            .chain((self.input_hotplug.registry != 0).then_some(super::input_hotplug::KEY))
            .chain((0..self.registry.list_packages().len()).map(|i| REGISTRY_BASE + i as u64))
            .chain((0..self.registry.active_pins().len()).map(|i| ACTIVE_PIN_BASE + i as u64))
            .chain((0..self.watchdogs.len()).map(|i| WATCHDOG_BASE + i as u64))
            .chain((0..self.services.len()).map(|i| SERVICE_BASE + i as u64))
            .chain((0..self.launches.len()).map(|i| LAUNCH_BASE + i as u64))
            .chain((0..self.opener_bindings.len()).map(|i| OPENER_BINDING_BASE + i as u64))
            .chain(
                (0..self.version_manager_bindings.len())
                    .map(|i| VERSION_MANAGER_BINDING_BASE + i as u64),
            )
            .chain(
                (0..self.app_manager_bindings.len()).map(|i| APP_MANAGER_BINDING_BASE + i as u64),
            )
            .chain(
                (0..self.worker_launcher_bindings.len())
                    .map(|i| WORKER_LAUNCHER_BINDING_BASE + i as u64),
            )
            .chain(
                (0..self.worker_policy_watchers.len())
                    .map(|i| WORKER_POLICY_WATCHER_BASE + i as u64),
            )
            .chain((0..self.component_configs.len()).map(|i| COMPONENT_CONFIG_BASE + i as u64))
            .collect()
    }
    fn encode_record(&self, key: u64) -> Result<Option<Vec<u8>>, Error> {
        let bytes = match key {
            super::shell::KEY => self.shell.encode(),
            COMMAND_STATE_KEY => self.commands.encode(),
            SERVICE_DIRECTORY_STATE_KEY => {
                encode_service_directory_bindings(&self.service_directory_bindings)
            }
            LAZY_STATE_KEY => self.lazy.encode(),
            super::input_hotplug::KEY => self.input_hotplug.encode(),
            0 => {
                let mut w = Encoder::new();
                for n in [
                    13,
                    self.vfsd.0,
                    self.sys_state_root.0,
                    self.active_slot as u64,
                    self.lifecycle.0,
                    self.updated_lifecycle.0,
                    self.users.0,
                    self.user_watcher.0,
                    self.migration.0,
                    self.generation,
                    self.registry.list_packages().len() as u64,
                    self.services.len() as u64,
                    self.launches.len() as u64,
                    self.opener_bindings.len() as u64,
                    self.version_manager_bindings.len() as u64,
                    self.app_manager_bindings.len() as u64,
                    self.worker_launcher_bindings.len() as u64,
                    self.worker_policy_watchers.len() as u64,
                    self.component_configs.len() as u64,
                    self.registry.active_pins().len() as u64,
                    self.watchdogs.len() as u64,
                    self.kernel_generation_floor,
                    self.tee_generation_floor,
                ] {
                    w.word(n);
                }
                w.finish()
            }
            1 => self.config_bytes.clone(),
            2 => self
                .broker
                .freeze()
                .encode()
                .map_err(|_| Error::InvalidData)?,
            4 => {
                return Ok(self.pending_record.as_ref().map(|r| {
                    let mut w = Encoder::new();
                    w.word(self.pending_archive.0);
                    w.word(self.pending_archive.1);
                    for h in [
                        self.pending_replacement.0,
                        self.pending_replacement.1,
                        self.pending_replacement.2,
                    ] {
                        w.word(h);
                    }
                    w.bytes(&r.checkpoint());
                    w.finish()
                }));
            }
            3 => {
                let mut w = Encoder::new();
                w.word(DEVICE_RECORD_MAGIC_V3);
                w.word(self.devices.nodes().len() as u64);
                for node in self.devices.nodes() {
                    w.word(node.info.node_id);
                    w.word(match node.info.bus {
                        crate::BusType::Pci => 1,
                        crate::BusType::Usb => 2,
                        crate::BusType::PlatformDt => 3,
                        crate::BusType::I2c => 4,
                        crate::BusType::Spi => 5,
                    });
                    match node.info.parent_node_id {
                        Some(parent) => {
                            w.word(1);
                            w.word(parent);
                        }
                        None => {
                            w.word(0);
                            w.word(0);
                        }
                    }
                    w.text(&node.info.topological_path);
                    w.word(node.info.properties.len() as u64);
                    for property in &node.info.properties {
                        w.text(&property.key);
                        w.word(property.value as u64);
                    }
                    w.word(node.resources.len() as u64);
                    for resource in &node.resources {
                        w.word(match resource.kind {
                            crate::HardwareResourceKind::Mmio => 1,
                            crate::HardwareResourceKind::Interrupt => 2,
                            crate::HardwareResourceKind::DmaPool => 3,
                            crate::HardwareResourceKind::IommuDomain => 4,
                            crate::HardwareResourceKind::RegisterProxy => 5,
                            crate::HardwareResourceKind::BusControl => 6,
                        });
                        w.word(resource.resource_id);
                        w.word(resource.base);
                        w.word(resource.length);
                        w.word(resource.flags);
                        w.word(resource.capability.object_id);
                        w.word(resource.capability.rights as u64);
                    }
                    w.word(node.present as u64);
                    encode_device_state(&mut w, &node.state);
                }
                w.finish()
            }
            5 => encode_openers(&self.openers),
            6 => crate::manager::encode_domain_associations(&self.domain_associations),
            crate::package_install::KEY => {
                crate::package_install::encode(&self.package_installs, self.package_retry_after)
            }
            DRIVER_LIFECYCLE_STATE_KEY => encode_driver_lifecycle_state(
                &self.driver_routes,
                &self.driver_recovery,
                &self.driver_images,
            ),
            PERMISSION_STATE_KEY => encode_permission_state(&self.permissions),
            PERMISSION_ROUTE_STATE_KEY => encode_permission_routes(&self.permission_routes),
            k if k >= OPENER_BINDING_BASE => {
                if k >= COMPONENT_CONFIG_BASE {
                    return Ok(self
                        .component_configs
                        .get((k - COMPONENT_CONFIG_BASE) as usize)
                        .map(ComponentConfigRecord::encode));
                }
                if k >= APP_MANAGER_BINDING_BASE {
                    if k >= WORKER_POLICY_WATCHER_BASE {
                        return Ok(self
                            .worker_policy_watchers
                            .get((k - WORKER_POLICY_WATCHER_BASE) as usize)
                            .map(|channel| {
                                let mut w = Encoder::new();
                                w.word(*channel);
                                w.finish()
                            }));
                    }
                    if k >= WORKER_LAUNCHER_BINDING_BASE {
                        return Ok(self
                            .worker_launcher_bindings
                            .get((k - WORKER_LAUNCHER_BINDING_BASE) as usize)
                            .map(crate::manager::encode_app_manager_binding));
                    }
                    return Ok(self
                        .app_manager_bindings
                        .get((k - APP_MANAGER_BINDING_BASE) as usize)
                        .map(crate::manager::encode_app_manager_binding));
                }
                if k >= VERSION_MANAGER_BINDING_BASE {
                    return Ok(self
                        .version_manager_bindings
                        .get((k - VERSION_MANAGER_BINDING_BASE) as usize)
                        .map(|channel| {
                            let mut w = Encoder::new();
                            w.word(*channel);
                            w.finish()
                        }));
                }
                return Ok(self
                    .opener_bindings
                    .get((k - OPENER_BINDING_BASE) as usize)
                    .map(encode_opener_binding));
            }
            k if k >= LAUNCH_BASE => {
                return Ok(self
                    .launches
                    .get((k - LAUNCH_BASE) as usize)
                    .map(LaunchRecord::encode));
            }
            k if k >= WATCHDOG_BASE && k < SERVICE_BASE => {
                return Ok(self
                    .watchdogs
                    .get((k - WATCHDOG_BASE) as usize)
                    .map(crate::watchdog::WatchdogRecord::checkpoint));
            }
            k if k >= ACTIVE_PIN_BASE && k < WATCHDOG_BASE => {
                return Ok(self
                    .registry
                    .active_pins()
                    .get((k - ACTIVE_PIN_BASE) as usize)
                    .map(ActivePinRecord::checkpoint));
            }
            k if k >= SERVICE_BASE => {
                return Ok(self
                    .services
                    .get((k - SERVICE_BASE) as usize)
                    .map(ManagedService::encode));
            }
            k if k >= REGISTRY_BASE => {
                return Ok(self
                    .registry
                    .list_packages()
                    .get((k - REGISTRY_BASE) as usize)
                    .map(AppRecord::checkpoint));
            }
            _ => return Err(Error::InvalidData),
        };
        Ok(Some(bytes))
    }
    fn adopt_record(&mut self, key: u64, bytes: Option<&[u8]>) -> Result<(), Error> {
        if key >= ACTIVE_PIN_BASE && key < WATCHDOG_BASE {
            match bytes {
                Some(bytes) => {
                    self.incoming_pins.insert(
                        key - ACTIVE_PIN_BASE,
                        ActivePinRecord::from_checkpoint(bytes)?,
                    );
                }
                None => {
                    self.incoming_pins.remove(&(key - ACTIVE_PIN_BASE));
                }
            }
            return Ok(());
        }
        if key >= REGISTRY_BASE && key < SERVICE_BASE {
            if key >= WATCHDOG_BASE {
                let i = usize::try_from(key - WATCHDOG_BASE).map_err(|_| Error::Capacity)?;
                if let Some(bytes) = bytes {
                    let record = crate::watchdog::WatchdogRecord::from_checkpoint(bytes)?;
                    if i > self.watchdogs.len() || i >= MAX_MANAGED_RUNTIME_ENTRIES {
                        return Err(Error::InvalidData);
                    }
                    if i == self.watchdogs.len() {
                        self.watchdogs.push(record);
                    } else {
                        self.watchdogs[i] = record;
                    }
                }
                return Ok(());
            }
            match bytes {
                Some(bytes) => {
                    self.incoming
                        .insert(key - REGISTRY_BASE, AppRecord::from_checkpoint(bytes)?);
                }
                None => {
                    self.incoming.remove(&(key - REGISTRY_BASE));
                }
            }
            return Ok(());
        }
        if key == 4 {
            self.pending_record = if let Some(bytes) = bytes {
                let mut r = Decoder::new(bytes);
                self.pending_archive = (r.word()?, r.word()?);
                self.pending_replacement = (r.word()?, r.word()?, r.word()?);
                let record = AppRecord::from_checkpoint(r.bytes(32704)?)?;
                r.finish()?;
                Some(record)
            } else {
                self.pending_archive = (0, 0);
                self.pending_replacement = (0, 0, 0);
                None
            };
            return Ok(());
        }
        let bytes = bytes.ok_or(Error::InvalidData)?;
        match key {
            super::shell::KEY => self.shell = crate::shell::Session::decode(bytes)?,
            COMMAND_STATE_KEY => self.commands = crate::command_state::CommandState::decode(bytes)?,
            SERVICE_DIRECTORY_STATE_KEY => {
                self.service_directory_bindings = decode_service_directory_bindings(bytes)?
            }
            LAZY_STATE_KEY => self.lazy = crate::lazy::LazyActivationState::decode(bytes)?,
            super::input_hotplug::KEY => {
                self.input_hotplug = super::input_hotplug::Hotplug::decode(bytes)?
            }
            0 => {
                let mut r = Decoder::new(bytes);
                let version = r.word()?;
                if !(1..=13).contains(&version) {
                    return Err(Error::UnsupportedVersion);
                }
                self.vfsd = Channel(r.word()?);
                if version >= 10 {
                    self.sys_state_root = Channel(r.word()?);
                    self.active_slot = match r.word()? {
                        0 => Slot::A,
                        1 => Slot::B,
                        _ => return Err(Error::InvalidData),
                    };
                } else {
                    self.sys_state_root = Channel(0);
                    self.active_slot = Slot::A;
                }
                self.lifecycle = Channel(r.word()?);
                self.updated_lifecycle = if version >= 13 {
                    Channel(r.word()?)
                } else {
                    Channel(0)
                };
                self.users = if version >= 2 {
                    Channel(r.word()?)
                } else {
                    Channel(0)
                };
                self.user_watcher = if version >= 11 {
                    Channel(r.word()?)
                } else {
                    Channel(0)
                };
                self.migration = Channel(r.word()?);
                self.generation = r.word()?;
                self.registry_count = r.count(1024)?;
                self.service_count = r.count(MAX_MANAGED_RUNTIME_ENTRIES)?;
                let launch_count = r.count(MAX_MANAGED_RUNTIME_ENTRIES)?;
                let opener_binding_count = if version >= 3 { r.count(64)? } else { 0 };
                let version_manager_binding_count = if version >= 4 { r.count(64)? } else { 0 };
                let app_manager_binding_count = if version >= 5 { r.count(64)? } else { 0 };
                let worker_launcher_binding_count = if version >= 6 { r.count(64)? } else { 0 };
                let worker_policy_watcher_count = if version >= 6 { r.count(64)? } else { 0 };
                let component_config_count = if version >= 7 {
                    r.count(MAX_COMPONENT_CONFIG_RECORDS)?
                } else {
                    0
                };
                self.active_pin_count = if version >= 9 { r.count(1024)? } else { 0 };
                self.watchdog_count = if version >= 9 {
                    r.count(MAX_MANAGED_RUNTIME_ENTRIES)?
                } else {
                    0
                };
                if version >= 12 {
                    self.kernel_generation_floor = r.word()?;
                    self.tee_generation_floor = r.word()?;
                }
                r.finish()?;
                self.launches.truncate(launch_count);
                self.opener_bindings.truncate(opener_binding_count);
                self.version_manager_bindings
                    .truncate(version_manager_binding_count);
                self.app_manager_bindings
                    .truncate(app_manager_binding_count);
                self.worker_launcher_bindings
                    .truncate(worker_launcher_binding_count);
                self.worker_policy_watchers
                    .truncate(worker_policy_watcher_count);
                self.component_configs.truncate(component_config_count);
                self.watchdogs.truncate(self.watchdog_count);
            }
            1 => {
                self.config = PlatformConfig::decode(bytes).map_err(|_| Error::InvalidData)?;
                self.config_bytes = bytes.to_vec();
            }
            2 => {
                self.broker = AppdBroker::restore(
                    AppdSnapshot::decode(bytes).map_err(|_| Error::InvalidData)?,
                )
                .map_err(|_| Error::InvalidData)?;
            }
            3 => {
                let mut r = Decoder::new(bytes);
                let mut devices = crate::DeviceRegistry::new();
                let first = r.word()?;
                let (version, count) = if first == DEVICE_RECORD_MAGIC_V3 {
                    (3, r.count(1024)?)
                } else if first == DEVICE_RECORD_MAGIC_V2 {
                    (2, r.count(1024)?)
                } else {
                    (1, usize::try_from(first).map_err(|_| Error::InvalidData)?)
                };
                for _ in 0..count {
                    let node_id = r.word()?;
                    let bus = match r.word()? {
                        1 => crate::BusType::Pci,
                        2 => crate::BusType::Usb,
                        3 => crate::BusType::PlatformDt,
                        4 => crate::BusType::I2c,
                        5 => crate::BusType::Spi,
                        _ => return Err(Error::InvalidData),
                    };
                    let parent_node_id = if version >= 2 {
                        if r.flag()? {
                            Some(r.word()?)
                        } else {
                            let _ = r.word()?;
                            None
                        }
                    } else {
                        None
                    };
                    let topological_path = if version >= 3 {
                        r.text(256)?.to_string()
                    } else if let Some(parent) = parent_node_id {
                        let parent_path = devices
                            .nodes()
                            .iter()
                            .find(|node| node.info.node_id == parent)
                            .map(|node| node.info.topological_path.as_str())
                            .ok_or(Error::InvalidData)?;
                        alloc::format!("{parent_path}/legacy-{node_id}")
                    } else {
                        alloc::format!("legacy-{node_id}")
                    };
                    let mut properties = Vec::new();
                    for _ in 0..r.count(64)? {
                        properties.push(crate::DeviceProperty {
                            key: r.text(128)?.to_string(),
                            value: u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?,
                        });
                    }
                    let mut resources = Vec::new();
                    let mut mmio_vmo = None;
                    let mut irq_channel = None;
                    if version >= 2 {
                        for _ in 0..r.count(16)? {
                            let kind = match r.word()? {
                                1 => crate::HardwareResourceKind::Mmio,
                                2 => crate::HardwareResourceKind::Interrupt,
                                3 => crate::HardwareResourceKind::DmaPool,
                                4 => crate::HardwareResourceKind::IommuDomain,
                                5 => crate::HardwareResourceKind::RegisterProxy,
                                6 => crate::HardwareResourceKind::BusControl,
                                _ => return Err(Error::InvalidData),
                            };
                            resources.push(crate::HardwareResourceLease {
                                kind,
                                resource_id: r.word()?,
                                base: r.word()?,
                                length: r.word()?,
                                flags: r.word()?,
                                capability: bexos_kernel_core::ipc::Capability {
                                    object_id: r.word()?,
                                    rights: u32::try_from(r.word()?)
                                        .map_err(|_| Error::InvalidData)?,
                                },
                            });
                        }
                    } else {
                        let mut cap =
                            || -> Result<Option<bexos_kernel_core::ipc::Capability>, Error> {
                                Ok(if r.flag()? {
                                    Some(bexos_kernel_core::ipc::Capability {
                                        object_id: r.word()?,
                                        rights: u32::try_from(r.word()?)
                                            .map_err(|_| Error::InvalidData)?,
                                    })
                                } else {
                                    None
                                })
                            };
                        mmio_vmo = cap()?;
                        irq_channel = cap()?;
                        if let Some(cap) = mmio_vmo {
                            resources.push(crate::HardwareResourceLease {
                                kind: crate::HardwareResourceKind::Mmio,
                                resource_id: 0,
                                base: 0,
                                length: 0,
                                flags: 0,
                                capability: cap,
                            });
                        }
                        if let Some(cap) = irq_channel {
                            resources.push(crate::HardwareResourceLease {
                                kind: crate::HardwareResourceKind::Interrupt,
                                resource_id: 1,
                                base: 0,
                                length: 0,
                                flags: 0,
                                capability: cap,
                            });
                        }
                    }
                    let present = if version >= 2 { r.flag()? } else { true };
                    let state = if version >= 2 {
                        decode_device_state(&mut r)?
                    } else {
                        Default::default()
                    };
                    devices
                        .restore_device_node(crate::RegisteredDeviceNode {
                            info: crate::DeviceNodeInfo {
                                node_id,
                                bus,
                                parent_node_id,
                                topological_path,
                                properties,
                            },
                            resources,
                            mmio_vmo,
                            irq_channel,
                            registrar: None,
                            present,
                            state,
                        })
                        .map_err(|_| Error::InvalidData)?;
                }
                r.finish()?;
                self.devices = devices;
            }
            5 => {
                self.openers = decode_openers(bytes)?;
            }
            6 => {
                self.domain_associations = crate::manager::decode_domain_associations(bytes)?;
            }
            crate::package_install::KEY => {
                (self.package_installs, self.package_retry_after) =
                    crate::package_install::decode(bytes)?;
            }
            DRIVER_LIFECYCLE_STATE_KEY => {
                let (routes, recovery, images) = decode_driver_lifecycle_state(bytes)?;
                self.driver_routes = routes;
                self.driver_recovery = recovery;
                self.driver_images = images;
            }
            PERMISSION_STATE_KEY => {
                self.permissions = decode_permission_state(bytes)?;
            }
            PERMISSION_ROUTE_STATE_KEY => {
                self.permission_routes = decode_permission_routes(bytes)?;
            }
            k if k >= OPENER_BINDING_BASE => {
                if k >= COMPONENT_CONFIG_BASE {
                    let i =
                        usize::try_from(k - COMPONENT_CONFIG_BASE).map_err(|_| Error::Capacity)?;
                    if i > self.component_configs.len() || i >= MAX_COMPONENT_CONFIG_RECORDS {
                        return Err(Error::InvalidData);
                    }
                    let record = ComponentConfigRecord::decode(bytes)?;
                    if i == self.component_configs.len() {
                        self.component_configs.push(record);
                    } else {
                        self.component_configs[i] = record;
                    }
                    return Ok(());
                }
                if k >= APP_MANAGER_BINDING_BASE {
                    if k >= WORKER_POLICY_WATCHER_BASE {
                        let i = usize::try_from(k - WORKER_POLICY_WATCHER_BASE)
                            .map_err(|_| Error::Capacity)?;
                        if i > self.worker_policy_watchers.len() || i >= 64 {
                            return Err(Error::InvalidData);
                        }
                        let mut r = Decoder::new(bytes);
                        let channel = r.word()?;
                        r.finish()?;
                        if channel == 0 {
                            return Err(Error::InvalidData);
                        }
                        if i == self.worker_policy_watchers.len() {
                            self.worker_policy_watchers.push(channel);
                        } else {
                            self.worker_policy_watchers[i] = channel;
                        }
                        return Ok(());
                    }
                    if k >= WORKER_LAUNCHER_BINDING_BASE {
                        let i = usize::try_from(k - WORKER_LAUNCHER_BINDING_BASE)
                            .map_err(|_| Error::Capacity)?;
                        if i > self.worker_launcher_bindings.len() || i >= 64 {
                            return Err(Error::InvalidData);
                        }
                        let binding = crate::manager::decode_app_manager_binding(bytes)?;
                        if i == self.worker_launcher_bindings.len() {
                            self.worker_launcher_bindings.push(binding);
                        } else {
                            self.worker_launcher_bindings[i] = binding;
                        }
                        return Ok(());
                    }
                    let i = usize::try_from(k - APP_MANAGER_BINDING_BASE)
                        .map_err(|_| Error::Capacity)?;
                    if i > self.app_manager_bindings.len() || i >= 64 {
                        return Err(Error::InvalidData);
                    }
                    let binding = crate::manager::decode_app_manager_binding(bytes)?;
                    if i == self.app_manager_bindings.len() {
                        self.app_manager_bindings.push(binding);
                    } else {
                        self.app_manager_bindings[i] = binding;
                    }
                    return Ok(());
                }
                if k >= VERSION_MANAGER_BINDING_BASE {
                    let i = usize::try_from(k - VERSION_MANAGER_BINDING_BASE)
                        .map_err(|_| Error::Capacity)?;
                    if i > self.version_manager_bindings.len() || i >= 64 {
                        return Err(Error::InvalidData);
                    }
                    let mut r = Decoder::new(bytes);
                    let channel = r.word()?;
                    r.finish()?;
                    if channel == 0 {
                        return Err(Error::InvalidData);
                    }
                    if i == self.version_manager_bindings.len() {
                        self.version_manager_bindings.push(channel);
                    } else {
                        self.version_manager_bindings[i] = channel;
                    }
                    return Ok(());
                }
                let i = usize::try_from(k - OPENER_BINDING_BASE).map_err(|_| Error::Capacity)?;
                let binding = decode_opener_binding(bytes)?;
                if i > self.opener_bindings.len() || i >= 64 {
                    return Err(Error::InvalidData);
                }
                if i == self.opener_bindings.len() {
                    self.opener_bindings.push(binding);
                } else {
                    self.opener_bindings[i] = binding;
                }
            }
            k if k >= LAUNCH_BASE => {
                let i = (k - LAUNCH_BASE) as usize;
                if i > self.launches.len() || i >= MAX_MANAGED_RUNTIME_ENTRIES {
                    return Err(Error::InvalidData);
                }
                let l = LaunchRecord::decode(bytes)?;
                if i == self.launches.len() {
                    self.launches.push(l);
                } else {
                    self.launches[i] = l;
                }
            }
            k if k >= SERVICE_BASE => {
                let i = usize::try_from(k - SERVICE_BASE).map_err(|_| Error::Capacity)?;
                let service = ManagedService::decode(bytes)?;
                if i > self.services.len() || i >= MAX_MANAGED_RUNTIME_ENTRIES {
                    return Err(Error::InvalidData);
                }
                if i == self.services.len() {
                    self.services.push(service);
                } else {
                    self.services[i] = service;
                }
            }
            _ => return Err(Error::InvalidData),
        }
        Ok(())
    }
    fn finish_adoption(&mut self) -> Result<(), Error> {
        let mut records = Vec::new();
        for i in 0..self.registry_count {
            records.push(
                self.incoming
                    .remove(&(i as u64))
                    .ok_or(Error::InvalidData)?,
            );
        }
        self.incoming.clear();
        let mut pins = Vec::new();
        for i in 0..self.active_pin_count {
            pins.push(
                self.incoming_pins
                    .remove(&(i as u64))
                    .ok_or(Error::InvalidData)?,
            );
        }
        self.incoming_pins.clear();
        self.registry = MemoryAppRegistry::from_checkpoint_records_and_pins(records, pins)?;
        if self.openers.system_handlers().is_empty() {
            let mut openers = MemoryOpenerRegistry::new();
            for record in self.registry.list_packages() {
                if let Ok(manifest) = crate::Manifest::decode(&record.manifest_bytes) {
                    register_manifest_openers(&mut openers, OpenerScope::System, &manifest, false);
                }
            }
            self.openers = openers;
        }
        if self.services.len() != self.service_count {
            return Err(Error::InvalidData);
        }
        if self.watchdogs.len() != self.watchdog_count {
            return Err(Error::InvalidData);
        }
        self.validate()
    }
    fn validate(&self) -> Result<(), Error> {
        if self.vfsd.0 == 0
            || self.lifecycle.0 == 0
            || self.users.0 == 0
            || self.migration.0 == 0
            || self.config_bytes.is_empty()
        {
            return Err(Error::InvalidData);
        }
        Ok(())
    }
    fn resources(&self) -> Vec<Resource> {
        let mut handles = alloc::vec![
            self.vfsd.0,
            self.sys_state_root.0,
            self.lifecycle.0,
            self.users.0,
            self.migration.0,
        ];
        handles.extend(
            self.package_installs
                .iter()
                .flat_map(|pending| [pending.resolver, pending.caller])
                .filter(|h| *h != 0),
        );
        handles.extend(self.shell.clients.iter().map(|c| c.channel));
        if self.shell.users_channel != 0 {
            handles.push(self.shell.users_channel);
        }
        handles.extend(
            self.shell
                .clients
                .iter()
                .map(|c| c.callback)
                .filter(|h| *h != 0),
        );
        if self.input_hotplug.registry != 0 {
            handles.push(self.input_hotplug.registry);
        }
        if self.user_watcher.0 != 0 {
            handles.push(self.user_watcher.0);
        }
        if self.updated_lifecycle.0 != 0 {
            handles.push(self.updated_lifecycle.0);
        }
        for s in &self.services {
            if s.package != APPD_PACKAGE {
                handles.extend([s.process_handle, s.space_handle, s.thread_handle]);
            }
            handles.extend([s.manager, s.migration, s.archive, s.resource_job]);
        }
        for route in self.permission_routes.routes() {
            handles.extend([
                route.retained_endpoint.object_id,
                route.client_endpoint.object_id,
                route.provider_manager.object_id,
            ]);
        }
        for node in self.devices.nodes() {
            for resource in &node.resources {
                handles.push(resource.capability.object_id);
            }
            match &node.state {
                crate::DeviceNodeState::Binding(binding)
                | crate::DeviceNodeState::Active(binding)
                | crate::DeviceNodeState::Quiescing(binding)
                | crate::DeviceNodeState::Suspended(binding) => {
                    if let Some(cap) = binding.process_handle {
                        handles.push(cap.object_id);
                    }
                    if let Some(cap) = binding.manager_channel {
                        handles.push(cap.object_id);
                    }
                    if let Some(cap) = binding.lifecycle_channel {
                        handles.push(cap.object_id);
                    }
                }
                crate::DeviceNodeState::Unbound | crate::DeviceNodeState::BindFailed { .. } => {}
            }
        }
        for image in self.driver_images.images() {
            handles.push(image.executable.handle);
            handles.extend(image.libraries.iter().map(|library| library.image.handle));
        }
        if self.pending_archive.0 != 0 {
            handles.push(self.pending_archive.0);
        }
        // Pending replacement descriptors are carried in record 4 so the
        // activated appd can update its registry after a self-transplant. They
        // describe the candidate process created for the kernel handover, not
        // source-owned resources that should be preserved through that handover.
        for l in &self.launches {
            handles.extend(l.handles());
        }
        handles.extend(self.commands.handles());
        for binding in &self.opener_bindings {
            handles.push(binding.channel);
        }
        handles.extend(self.version_manager_bindings.iter().copied());
        handles.extend(
            self.app_manager_bindings
                .iter()
                .map(|binding| binding.channel),
        );
        handles.extend(
            self.worker_launcher_bindings
                .iter()
                .map(|binding| binding.channel),
        );
        handles.extend(
            self.service_directory_bindings
                .iter()
                .map(|binding| binding.channel),
        );
        handles.extend(self.worker_policy_watchers.iter().copied());
        handles.extend(self.lazy.handles());
        handles.retain(|h| *h != 0);
        handles.sort_unstable();
        handles.dedup();
        handles.into_iter().map(Resource::Handle).collect()
    }
    fn activated(&mut self, generation: u64) {
        self.generation = generation;
        if let Err(error) = self.restore_persistence_after_handover() {
            bexos_userspace::log(&alloc::format!(
                "appd: migrated registry activation failed {error:?}\n"
            ));
            bexos_userspace::exit();
        }
        if let Some(s) = self.services.iter_mut().find(|s| s.package == APPD_PACKAGE) {
            if self.pending_replacement.0 != 0 {
                (s.process_handle, s.space_handle, s.thread_handle) = self.pending_replacement;
            }
            s.generation = generation;
            while let Ok(message) = Channel(s.migration).try_recv() {
                for handle in message.handles {
                    let _ = bexos_userspace::Memory::close(handle);
                }
            }
        }
        self.pending_replacement = (0, 0, 0);
        bexos_userspace::log(&alloc::format!(
            "appd: replacement generation={generation}; existing lifecycle endpoint adopted\n"
        ));
    }
}

fn encode_openers(openers: &MemoryOpenerRegistry) -> Vec<u8> {
    let mut w = Encoder::new();
    let system = openers.system_handlers();
    let users = openers.user_handlers();
    let defaults = openers.user_defaults();
    w.word(system.len() as u64);
    for registration in system {
        encode_registration(&mut w, registration);
    }
    w.word(users.len() as u64);
    for entry in users {
        w.word(entry.uid);
        encode_registration(&mut w, &entry.registration);
    }
    w.word(defaults.len() as u64);
    for entry in defaults {
        w.word(entry.uid);
        w.text(&entry.key);
        w.text(&entry.handler.package);
        w.text(&entry.handler.process);
    }
    w.finish()
}

fn decode_openers(bytes: &[u8]) -> Result<MemoryOpenerRegistry, Error> {
    let mut r = Decoder::new(bytes);
    let mut openers = MemoryOpenerRegistry::new();
    for _ in 0..r.count(1024)? {
        openers
            .register(OpenerScope::System, decode_registration(&mut r)?)
            .map_err(|_| Error::InvalidData)?;
    }
    for _ in 0..r.count(1024)? {
        let uid = r.word()?;
        openers
            .register(OpenerScope::User(uid), decode_registration(&mut r)?)
            .map_err(|_| Error::InvalidData)?;
    }
    for _ in 0..r.count(1024)? {
        let uid = r.word()?;
        let key = r.text(128)?.to_string();
        let package = r.text(128)?.to_string();
        let process = r.text(64)?.to_string();
        openers
            .set_user_default(uid, key, HandlerId::new(package, process))
            .map_err(|_| Error::InvalidData)?;
    }
    r.finish()?;
    Ok(openers)
}

fn encode_registration(w: &mut Encoder, registration: &HandlerRegistration) {
    w.text(&registration.package);
    w.text(&registration.process);
    encode_strings(w, &registration.schemes);
    encode_strings(w, &registration.domains);
    encode_strings(w, &registration.mime_types);
    encode_strings(w, &registration.interfaces);
    w.word(registration.domains_verified as u64);
}

fn decode_registration(r: &mut Decoder<'_>) -> Result<HandlerRegistration, Error> {
    Ok(HandlerRegistration {
        package: r.text(128)?.to_string(),
        process: r.text(64)?.to_string(),
        schemes: decode_strings(r, 64)?,
        domains: decode_strings(r, 64)?,
        mime_types: decode_strings(r, 64)?,
        interfaces: decode_strings(r, 64)?,
        domains_verified: r.flag()?,
    })
}

fn encode_strings(w: &mut Encoder, values: &[String]) {
    w.word(values.len() as u64);
    for value in values {
        w.text(value);
    }
}

fn decode_strings(r: &mut Decoder<'_>, max: usize) -> Result<Vec<String>, Error> {
    let mut out = Vec::new();
    for _ in 0..r.count(max)? {
        out.push(r.text(128)?.to_string());
    }
    Ok(out)
}

fn encode_opener_binding(binding: &OpenerBinding) -> Vec<u8> {
    let mut w = Encoder::new();
    w.word(binding.channel);
    w.text(&binding.package);
    w.word(binding.uid);
    w.word(binding.system as u64);
    w.finish()
}

fn decode_opener_binding(bytes: &[u8]) -> Result<OpenerBinding, Error> {
    let mut r = Decoder::new(bytes);
    let binding = OpenerBinding {
        channel: r.word()?,
        package: r.text(128)?.to_string(),
        uid: r.word()?,
        system: r.flag()?,
    };
    r.finish()?;
    if binding.channel == 0 || binding.package.is_empty() {
        return Err(Error::InvalidData);
    }
    Ok(binding)
}

fn encode_service_directory_bindings(bindings: &[crate::ServiceDirectoryBinding]) -> Vec<u8> {
    let mut w = Encoder::new();
    w.word(bindings.len() as u64);
    for binding in bindings {
        w.bytes(&crate::service_directory::encode_service_directory_binding(
            binding,
        ));
    }
    w.finish()
}

fn decode_service_directory_bindings(
    bytes: &[u8],
) -> Result<Vec<crate::ServiceDirectoryBinding>, Error> {
    let mut r = Decoder::new(bytes);
    let mut bindings = Vec::new();
    for _ in 0..r.count(64)? {
        bindings.push(crate::service_directory::decode_service_directory_binding(
            r.bytes(1024)?,
        )?);
    }
    r.finish()?;
    Ok(bindings)
}
