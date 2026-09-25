#![no_std]

//! Stable SDK surface for native component startup and live migration.

pub use bexos_libc::entry as std_entry;
pub use bexos_migration as migration_codec;
pub use bexos_userspace::{
    Channel, HardwareResourceKind, NamespaceEntry, ServiceGrant, Startup, StartupHardwareResource,
    entry, exit, live_migration, log, migration, service_binding, yield_now,
};

/// Version implemented by the SDK migration codec.
pub const MIGRATION_STATE_VERSION: u64 = bexos_migration::VERSION;
