//! Reopen process-local database objects after their source owner is retired.
use super::{AppdState, Error};

impl AppdState {
    pub(super) fn restore_persistence_after_handover(&mut self) -> Result<(), Error> {
        #[cfg(feature = "persistent")]
        {
            // Opening redb updates recovery metadata. Batch these writes with
            // activation instead of flushing the whole BexFS volume per store.
            // Both volumes must be durable before the replacement serves.
            let deferral = bexos_redb::bexos_fs::defer_file_syncs();
            let directory =
                bexos_userspace::vfs::get_system_data_directory(self.vfsd, super::APPD_PACKAGE)
                    .map_err(|_| Error::BadState)?;
            let result = (|| {
                if self.stores.is_none() {
                    self.stores = Some(
                        crate::stores::AppdStores::open(
                            directory,
                            self.sys_state_root,
                            self.active_slot,
                        )
                        .map_err(|error| {
                            bexos_userspace::log(&alloc::format!(
                                "appd: migrated stores reopen failed {error:?}\n"
                            ));
                            Error::BadState
                        })?,
                    );
                }
                self.activate_record()?;
                if let Some(stores) = &self.stores {
                    stores
                        .replace_from_memory(
                            &self.registry,
                            &self.openers,
                            &self.domain_associations,
                        )
                        .map_err(|_| Error::BadState)?;
                }
                Ok(())
            })();
            drop(deferral);
            let result = result.and_then(|()| {
                bexos_userspace::fs::sync(directory).map_err(|_| Error::BadState)?;
                bexos_userspace::fs::sync(self.sys_state_root).map_err(|_| Error::BadState)
            });
            let _ = bexos_userspace::Memory::close(directory.0);
            result
        }
        #[cfg(not(feature = "persistent"))]
        self.activate_record()
    }
}
