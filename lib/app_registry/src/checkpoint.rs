//! Canonical registry records for appd migration, including protection policy.
use super::*;
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};

impl AppRecord {
    pub fn checkpoint(&self) -> Vec<u8> {
        let mut w = Encoder::new();
        w.word(5);
        w.text(&self.package_id);
        w.text(&self.display_name);
        w.bytes(&self.manifest_bytes);
        encode_semver(&mut w, &self.version);
        w.word(self.multi_version_policy.to_u8() as u64);
        w.word(self.min_bexos_abi_version as u64);
        w.bytes(&self.archive_content_root);
        w.bytes(&self.signer_key_id);
        w.word(match self.install_source {
            InstallSource::Bootfs => 1,
            InstallSource::SystemImage => 2,
            InstallSource::Debugd => 3,
            InstallSource::Oci => 4,
        });
        w.word(match self.lifecycle_state {
            LifecycleState::Installed => 1,
            LifecycleState::Launching => 2,
            LifecycleState::Running => 3,
            LifecycleState::Stopped => 4,
        });
        w.word(self.protected as u64);
        w.text(&self.archive_path);
        w.word(self.package_instance_id);
        if let Some(signer) = &self.verified_signer {
            w.word(1);
            w.text(&signer.root_anchor_id);
            w.bytes(&signer.leaf_certificate_fingerprint);
            w.word(signer.signature_algorithm as u64);
            w.word(signer.granted_trust_tier as u64);
        } else {
            w.word(0);
        }
        w.word(self.accepted_generation);
        w.finish()
    }
    pub fn from_checkpoint(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = Decoder::new(bytes);
        let checkpoint_version = r.word()?;
        if !(1..=5).contains(&checkpoint_version) {
            return Err(Error::UnsupportedVersion);
        }
        let package_id = r.text(128)?.to_string();
        let display_name = r.text(128)?.to_string();
        let manifest_bytes = r.bytes(32768)?.to_vec();
        let (version, multi_version_policy, min_bexos_abi_version) = if checkpoint_version >= 2 {
            (
                decode_semver_from_checkpoint(&mut r)?,
                MultiVersionPolicy::from_u8(
                    u8::try_from(r.word()?).map_err(|_| Error::InvalidData)?,
                )
                .ok_or(Error::InvalidData)?,
                u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?,
            )
        } else {
            let (_, _, version, multi_version_policy, min_bexos_abi_version) =
                decode_manifest_header_unchecked(&manifest_bytes)
                    .map_err(|_| Error::InvalidData)?;
            (version, multi_version_policy, min_bexos_abi_version)
        };
        let archive_content_root = r.bytes(32)?.try_into().map_err(|_| Error::InvalidData)?;
        let signer_key_id = r.bytes(32)?.try_into().map_err(|_| Error::InvalidData)?;
        let install_source = match r.word()? {
            1 => InstallSource::Bootfs,
            2 => InstallSource::SystemImage,
            3 => InstallSource::Debugd,
            4 => InstallSource::Oci,
            _ => return Err(Error::InvalidData),
        };
        let lifecycle_state = match r.word()? {
            1 => LifecycleState::Installed,
            2 => LifecycleState::Launching,
            3 => LifecycleState::Running,
            4 => LifecycleState::Stopped,
            _ => return Err(Error::InvalidData),
        };
        let protected = r.flag()?;
        let archive_path = r.text(256)?.to_string();
        let package_instance_id = if checkpoint_version >= 3 {
            r.word()?
        } else {
            0
        };
        let verified_signer = if checkpoint_version >= 4 && r.flag()? {
            let root_anchor_id = r.text(64)?.to_string();
            let leaf_certificate_fingerprint =
                r.bytes(32)?.try_into().map_err(|_| Error::InvalidData)?;
            let signature_algorithm = u8::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
            let granted_trust_tier = u8::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
            Some(VerifiedSignerMetadata {
                root_anchor_id,
                leaf_certificate_fingerprint,
                signature_algorithm,
                granted_trust_tier,
            })
        } else if install_source == InstallSource::Bootfs
            || install_source == InstallSource::SystemImage
        {
            Some(product_trust_metadata())
        } else {
            None
        };
        let accepted_generation = if checkpoint_version >= 5 {
            r.word()?
        } else {
            0
        };
        r.finish()?;
        validate_package_id(&package_id).map_err(|_| Error::InvalidData)?;
        if install_source == InstallSource::Bootfs
            && archive_path.starts_with("/boot/")
            && !archive_path.contains("..")
        {
            // BootFS manifest-cache records retain their absolute boot paths.
        } else {
            validate_archive_path(&archive_path).map_err(|_| Error::InvalidData)?;
        }
        let (manifest_package, _, _, _, _) =
            decode_manifest_header_unchecked(&manifest_bytes).map_err(|_| Error::InvalidData)?;
        if manifest_package != package_id {
            return Err(Error::InvalidData);
        }
        let package_instance_id = if package_instance_id == 0 {
            installation_instance_id(
                &manifest_bytes,
                archive_content_root,
                signer_key_id,
                &archive_path,
            )
        } else {
            package_instance_id
        };
        Ok(Self {
            package_id,
            display_name,
            manifest_bytes,
            version,
            multi_version_policy,
            min_bexos_abi_version,
            archive_content_root,
            signer_key_id,
            verified_signer,
            install_source,
            lifecycle_state,
            protected,
            archive_path,
            package_instance_id,
            accepted_generation,
        })
    }
}

impl ActivePinRecord {
    pub fn checkpoint(&self) -> Vec<u8> {
        let mut w = Encoder::new();
        w.word(1);
        w.text(&self.package_id);
        encode_semver(&mut w, &self.pinned_version);
        w.bytes(&self.content_blake3);
        w.word(self.is_critical_boot_app as u64);
        w.word(self.health_check_status.to_u8() as u64);
        if let Some(version) = &self.rollback_target_version {
            w.word(1);
            encode_semver(&mut w, version);
        } else {
            w.word(0);
        }
        w.finish()
    }

    pub fn from_checkpoint(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = Decoder::new(bytes);
        let checkpoint_version = r.word()?;
        if checkpoint_version != 1 {
            return Err(Error::UnsupportedVersion);
        }
        let package_id = r.text(128)?.to_string();
        let pinned_version = decode_semver_from_checkpoint(&mut r)?;
        let content_blake3 = r.bytes(32)?.try_into().map_err(|_| Error::InvalidData)?;
        let is_critical_boot_app = r.flag()?;
        let health_check_status =
            HealthCheckStatus::from_u8(u8::try_from(r.word()?).map_err(|_| Error::InvalidData)?)
                .ok_or(Error::InvalidData)?;
        let rollback_target_version = if r.flag()? {
            Some(decode_semver_from_checkpoint(&mut r)?)
        } else {
            None
        };
        r.finish()?;
        validate_package_id(&package_id).map_err(|_| Error::InvalidData)?;
        Ok(Self {
            package_id,
            pinned_version,
            content_blake3,
            is_critical_boot_app,
            health_check_status,
            rollback_target_version,
        })
    }
}

impl MemoryAppRegistry {
    /// Replace the currently managed record for a package after validating the
    /// new checkpoint. Existing records were validated when installed and do
    /// not need to be serialized and revalidated as a side effect of updating
    /// an unrelated package.
    pub fn replace_checkpoint_record(&mut self, record: AppRecord) -> Result<(), Error> {
        record
            .validate_architecture()
            .map_err(|_| Error::InvalidData)?;
        AppRecord::from_checkpoint(&record.checkpoint())?;
        let previous = self
            .record(&record.package_id)
            .map_err(|_| Error::InvalidData)?;
        if (previous.protected && !record.protected)
            || record.accepted_generation < previous.accepted_generation
        {
            return Err(Error::InvalidData);
        }
        let mut records = self.records.clone();
        if let Some(existing) = records
            .iter_mut()
            .find(|existing| existing.package_key() == record.package_key())
        {
            *existing = record.clone();
        } else {
            records.push(record.clone());
        }
        records.sort_by_key(|record| record.package_key());
        if records
            .windows(2)
            .any(|pair| pair[0].package_key() >= pair[1].package_key())
        {
            return Err(Error::InvalidData);
        }
        let mut pins = self.active_pins.clone();
        if let Some(pin) = pins
            .iter_mut()
            .find(|pin| pin.package_id == record.package_id)
        {
            if pin.pinned_version != record.version {
                pin.rollback_target_version = Some(pin.pinned_version.clone());
            }
            pin.pinned_version = record.version.clone();
            pin.content_blake3 = record.archive_content_root;
            pin.is_critical_boot_app = record.protected;
        }
        validate_active_pins(&records, &pins).map_err(|_| Error::InvalidData)?;
        self.records = records;
        self.active_pins = pins;
        Ok(())
    }

    pub fn from_checkpoint_records(records: Vec<AppRecord>) -> Result<Self, Error> {
        Self::from_checkpoint_records_and_pins(records, Vec::new())
    }

    pub fn from_checkpoint_records_and_pins(
        records: Vec<AppRecord>,
        pins: Vec<ActivePinRecord>,
    ) -> Result<Self, Error> {
        if records
            .windows(2)
            .any(|r| r[0].package_key() >= r[1].package_key())
        {
            return Err(Error::InvalidData);
        }
        for record in &records {
            AppRecord::from_checkpoint(&record.checkpoint())?;
        }
        let mut registry = Self {
            records,
            active_pins: Vec::new(),
        };
        if pins.is_empty() {
            let records = registry.records.clone();
            for record in records {
                if record.multi_version_policy == MultiVersionPolicy::SingleActiveOnly {
                    registry.pin_active_from_record(&record);
                }
            }
        } else {
            registry
                .replace_active_pins(pins)
                .map_err(|_| Error::InvalidData)?;
        }
        Ok(registry)
    }
}

fn encode_semver(w: &mut Encoder, version: &SemVer) {
    w.word(version.major as u64);
    w.word(version.minor as u64);
    w.word(version.patch as u64);
    w.word(version.build as u64);
    w.text(&version.prerelease);
}

fn decode_semver_from_checkpoint(r: &mut Decoder<'_>) -> Result<SemVer, Error> {
    Ok(SemVer {
        major: u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?,
        minor: u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?,
        patch: u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?,
        build: u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?,
        prerelease: r.text(64)?.to_string(),
    })
}
