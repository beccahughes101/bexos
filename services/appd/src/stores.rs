use bexos_app_registry::{MemoryAppRegistry, persistent::AppRegistryDb};
use bexos_bexfs::sys_state::{ActivePackagePin, Slot, pinned_apps_path};
use bexos_domain_association::{MemoryDomainAssociationCache, persistent::DomainAssociationDb};
use bexos_opener_store::{MemoryOpenerRegistry, persistent::OpenerStoreDb};
use bexos_package_version::SemVer;
use bexos_redb::bexos_fs::FileBlockStore;
use bexos_userspace::{Channel, fs};
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};

pub const APP_REGISTRY_DB: &str = "app_registry.redb";
pub const DOMAIN_ASSOCIATIONS_DB: &str = "domain_associations.redb";
pub const PERMISSIONS_DB: &str = "permissions.redb";
pub const OPENERS_DB: &str = "openers.redb";
pub const GENERATION_FLOORS_DB: &str = "generation_floors.redb";

pub struct AppdStores {
    pub app_registry: AppRegistryDb,
    pub active_pins: ActivePinDb,
    pub domain_associations: DomainAssociationDb,
    pub openers: OpenerStoreDb,
    pub generation_floors: GenerationFloorDb,
}

impl AppdStores {
    pub fn open(
        system_dir: Channel,
        sys_state_root: Channel,
        active_slot: Slot,
    ) -> Result<Self, StoreError> {
        Ok(Self {
            app_registry: AppRegistryDb::open(open_db(system_dir, APP_REGISTRY_DB)?)
                .map_err(|_| StoreError::AppRegistry)?,
            active_pins: ActivePinDb::open(open_db(sys_state_root, pinned_apps_path(active_slot))?)
                .map_err(|_| StoreError::ActivePins)?,
            domain_associations: DomainAssociationDb::open(open_db(
                system_dir,
                DOMAIN_ASSOCIATIONS_DB,
            )?)
            .map_err(|_| StoreError::DomainAssociations)?,
            openers: OpenerStoreDb::open(open_db(system_dir, OPENERS_DB)?)
                .map_err(|_| StoreError::Openers)?,
            generation_floors: GenerationFloorDb::open(open_db(system_dir, GENERATION_FLOORS_DB)?)
                .map_err(|_| StoreError::GenerationFloors)?,
        })
    }

    pub fn replace_from_memory(
        &self,
        registry: &MemoryAppRegistry,
        openers: &MemoryOpenerRegistry,
        domain_associations: &MemoryDomainAssociationCache,
    ) -> Result<(), StoreError> {
        self.app_registry
            .replace_from_memory(registry)
            .map_err(|_| StoreError::AppRegistry)?;
        self.active_pins
            .replace_from_memory(registry)
            .map_err(|_| StoreError::ActivePins)?;
        self.openers
            .replace_from_memory(openers)
            .map_err(|_| StoreError::Openers)?;
        self.domain_associations
            .replace_from_memory(domain_associations)
            .map_err(|_| StoreError::DomainAssociations)?;
        Ok(())
    }

    pub fn snapshot_memory(
        &self,
    ) -> Result<
        (
            MemoryAppRegistry,
            MemoryOpenerRegistry,
            MemoryDomainAssociationCache,
        ),
        StoreError,
    > {
        Ok((
            self.app_registry
                .snapshot_memory()
                .map_err(|_| StoreError::AppRegistry)?,
            self.openers
                .snapshot_memory()
                .map_err(|_| StoreError::Openers)?,
            self.domain_associations
                .snapshot_memory()
                .map_err(|_| StoreError::DomainAssociations)?,
        ))
    }

    pub fn reopen_with_boot_state(
        &self,
        registry: &mut MemoryAppRegistry,
        openers: &mut MemoryOpenerRegistry,
        domain_associations: &mut MemoryDomainAssociationCache,
    ) -> Result<(), StoreError> {
        let (mut durable_registry, mut durable_openers, mut durable_domains) =
            self.snapshot_memory()?;
        let initialized_pins = self
            .active_pins
            .snapshot_pins()
            .map_err(|_| StoreError::ActivePins)?;
        for record in registry.list_packages() {
            if durable_registry.record(&record.package_key()).is_err() {
                durable_registry.install_checkpoint_record(record.clone());
            }
        }
        if self
            .active_pins
            .initialized()
            .map_err(|_| StoreError::ActivePins)?
        {
            durable_registry
                .replace_active_pins(initialized_pins)
                .map_err(|_| StoreError::ActivePins)?;
        }
        durable_openers.merge_from_memory(openers);
        for (domain, record) in domain_associations.records() {
            durable_domains
                .upsert(domain, record.clone())
                .map_err(|_| StoreError::DomainAssociations)?;
        }
        self.replace_from_memory(&durable_registry, &durable_openers, &durable_domains)?;
        *registry = durable_registry;
        *openers = durable_openers;
        *domain_associations = durable_domains;
        Ok(())
    }
}

const PIN_META: TableDefinition<&str, u64> = TableDefinition::new("metadata");
const PIN_RECORDS: TableDefinition<&str, &[u8]> = TableDefinition::new("active_pins");
const PIN_INIT_KEY: &str = "initialized";

pub struct ActivePinDb {
    db: Database,
}

impl ActivePinDb {
    pub fn open(store: FileBlockStore) -> Result<Self, StoreError> {
        Ok(Self {
            db: bexos_redb::open_or_create_with_store(store).map_err(|_| StoreError::ActivePins)?,
        })
    }

    pub fn initialized(&self) -> Result<bool, StoreError> {
        let tx = self.db.begin_read().map_err(|_| StoreError::ActivePins)?;
        let Ok(table) = tx.open_table(PIN_META) else {
            return Ok(false);
        };
        Ok(table
            .get(PIN_INIT_KEY)
            .map_err(|_| StoreError::ActivePins)?
            .is_some())
    }

    pub fn snapshot_pins(&self) -> Result<Vec<bexos_app_registry::ActivePinRecord>, StoreError> {
        let tx = self.db.begin_read().map_err(|_| StoreError::ActivePins)?;
        let Ok(table) = tx.open_table(PIN_RECORDS) else {
            return Ok(Vec::new());
        };
        table
            .iter()
            .map_err(|_| StoreError::ActivePins)?
            .map(|entry| {
                let (_, value) = entry.map_err(|_| StoreError::ActivePins)?;
                let pin =
                    ActivePackagePin::decode(value.value()).map_err(|_| StoreError::ActivePins)?;
                Ok(bexos_app_registry::ActivePinRecord {
                    package_id: pin.package_id,
                    pinned_version: pin.pinned_version,
                    content_blake3: pin.content_blake3,
                    is_critical_boot_app: pin.is_critical_boot_app,
                    health_check_status: pin.health_check_status,
                    rollback_target_version: pin.rollback_target_version,
                })
            })
            .collect()
    }

    pub fn replace_from_memory(&self, registry: &MemoryAppRegistry) -> Result<(), StoreError> {
        let tx = self.db.begin_write().map_err(|_| StoreError::ActivePins)?;
        {
            let mut meta = tx
                .open_table(PIN_META)
                .map_err(|_| StoreError::ActivePins)?;
            meta.insert(PIN_INIT_KEY, 1)
                .map_err(|_| StoreError::ActivePins)?;
        }
        {
            let mut table = tx
                .open_table(PIN_RECORDS)
                .map_err(|_| StoreError::ActivePins)?;
            table
                .retain(|_, _| false)
                .map_err(|_| StoreError::ActivePins)?;
            for pin in registry.active_pins() {
                let encoded = ActivePackagePin {
                    package_id: pin.package_id.clone(),
                    pinned_version: SemVer {
                        major: pin.pinned_version.major,
                        minor: pin.pinned_version.minor,
                        patch: pin.pinned_version.patch,
                        build: pin.pinned_version.build,
                        prerelease: pin.pinned_version.prerelease.clone(),
                    },
                    content_blake3: pin.content_blake3,
                    is_critical_boot_app: pin.is_critical_boot_app,
                    health_check_status: pin.health_check_status,
                    rollback_target_version: pin.rollback_target_version.clone(),
                }
                .encode();
                table
                    .insert(pin.package_id.as_str(), encoded.as_slice())
                    .map_err(|_| StoreError::ActivePins)?;
            }
        }
        tx.commit().map_err(|_| StoreError::ActivePins)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoreError {
    Open,
    AppRegistry,
    ActivePins,
    DomainAssociations,
    Permissions,
    Openers,
    GenerationFloors,
}

fn open_db(system_dir: Channel, name: &str) -> Result<FileBlockStore, StoreError> {
    if let Some((parent, leaf)) = name.rsplit_once('/') {
        let parent = fs::open(system_dir, parent, 1 | 2 | 8 | 32).map_err(|_| StoreError::Open)?;
        let file = fs::open(parent, leaf, 1 | 2 | 8).map_err(|_| StoreError::Open)?;
        let _ = fs::close(parent);
        return Ok(FileBlockStore::new(file));
    }
    fs::open(system_dir, name, 1 | 2 | 8)
        .map(FileBlockStore::new)
        .map_err(|_| StoreError::Open)
}

const FLOORS: TableDefinition<&str, u64> = TableDefinition::new("generation_floors");

pub struct GenerationFloorDb {
    db: Database,
}

impl GenerationFloorDb {
    pub fn open(store: FileBlockStore) -> Result<Self, StoreError> {
        Ok(Self {
            db: bexos_redb::open_or_create_with_store(store)
                .map_err(|_| StoreError::GenerationFloors)?,
        })
    }

    pub fn get(&self, target: &str) -> Result<u64, StoreError> {
        let tx = self
            .db
            .begin_read()
            .map_err(|_| StoreError::GenerationFloors)?;
        let Ok(table) = tx.open_table(FLOORS) else {
            return Ok(0);
        };
        Ok(table
            .get(target)
            .map_err(|_| StoreError::GenerationFloors)?
            .map_or(0, |value| value.value()))
    }

    pub fn commit_monotonic(&self, target: &str, generation: u64) -> Result<u64, StoreError> {
        let tx = self
            .db
            .begin_write()
            .map_err(|_| StoreError::GenerationFloors)?;
        let current = {
            let table = tx
                .open_table(FLOORS)
                .map_err(|_| StoreError::GenerationFloors)?;
            table
                .get(target)
                .map_err(|_| StoreError::GenerationFloors)?
                .map_or(0, |value| value.value())
        };
        let committed = current.max(generation);
        {
            let mut table = tx
                .open_table(FLOORS)
                .map_err(|_| StoreError::GenerationFloors)?;
            table
                .insert(target, committed)
                .map_err(|_| StoreError::GenerationFloors)?;
        }
        tx.commit().map_err(|_| StoreError::GenerationFloors)?;
        Ok(committed)
    }
}
