extern crate alloc;

pub mod guest;

use alloc::format;
use alloc::string::String;
use bexos_package_version::{MultiVersionPolicy, SemVer, split_versioned_package};
use fs_fidl::FsStatus;

pub fn package_archive_path(package_id: &str) -> Result<String, FsStatus> {
    validate_package_id(package_id)?;
    if let Some((package, version)) = split_versioned_package(package_id) {
        validate_package_id(package)?;
        validate_version_label(version)?;
        return Ok(format!("pkg/{package}/{version}/pkg.bex"));
    }
    Ok(format!("pkg/{package_id}.bex"))
}

/// Generation archives are staged by appd and made durable with the registry
/// record in its later namespace commit. Keep this syntax narrow so an ordinary
/// package name containing `generation` cannot opt out of normal durability.
pub fn is_staged_generation_archive(package_id: &str) -> bool {
    let Some((package, suffix)) = package_id.rsplit_once(".generation_") else {
        return false;
    };
    let Some((generation, digest)) = suffix.split_once('.') else {
        return false;
    };
    !package.is_empty()
        && !generation.is_empty()
        && generation.bytes().all(|byte| byte.is_ascii_digit())
        && digest.len() == 32
        && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub fn update_archive_path(archive_id: &str) -> Result<String, FsStatus> {
    validate_package_id(archive_id)?;
    Ok(format!("updates/{archive_id}.bex"))
}

pub fn app_data_directory_path(uid: u64, package_id: &str) -> Result<String, FsStatus> {
    if let Some(package) = package_id.strip_suffix(":shared") {
        validate_package_id(package)?;
        return Ok(format!("data/users/{uid}/apps/{package}/shared/data"));
    }
    if let Some((package, version)) = split_versioned_package(package_id) {
        validate_package_id(package)?;
        validate_version_label(version)?;
        return Ok(format!("data/users/{uid}/apps/{package}/{version}/data"));
    }
    app_data_directory_path_for_policy(
        uid,
        package_id,
        &SemVer::default(),
        MultiVersionPolicy::SingleActiveOnly,
    )
}

pub fn app_data_directory_path_for_policy(
    uid: u64,
    package_id: &str,
    version: &SemVer,
    policy: MultiVersionPolicy,
) -> Result<String, FsStatus> {
    validate_package_id(package_id)?;
    match policy {
        MultiVersionPolicy::ParallelExecution if !version.is_legacy() => Ok(format!(
            "data/users/{uid}/apps/{package_id}/{}/data",
            version.label()
        )),
        MultiVersionPolicy::SharedStorageMulti => {
            Ok(format!("data/users/{uid}/apps/{package_id}/shared/data"))
        }
        _ => Ok(format!("data/users/{uid}/apps/{package_id}/data")),
    }
}

pub fn user_filesystem_path(uid: u64) -> Result<String, FsStatus> {
    validate_uid(uid)?;
    Ok(format!("data/users/{uid}"))
}

pub fn system_data_directory_path(package_id: &str) -> Result<String, FsStatus> {
    validate_package_id(package_id)?;
    Ok(format!("data/system/{package_id}"))
}

pub fn shared_vault_directory_path(
    uid: u64,
    domain: &str,
    vault_name: &str,
    system: bool,
) -> Result<String, FsStatus> {
    validate_shared_vault_name(vault_name)?;
    if system {
        if !domain.is_empty() {
            return Err(FsStatus::InvalidArgs);
        }
        return Ok(format!("data/system/shared/local/{vault_name}/data"));
    }
    validate_uid(uid)?;
    if domain.is_empty() {
        Ok(format!("data/users/{uid}/shared/local/{vault_name}/data"))
    } else {
        validate_domain(domain)?;
        Ok(format!(
            "data/users/{uid}/shared/domain/{domain}/{vault_name}/data"
        ))
    }
}

fn validate_uid(_uid: u64) -> Result<(), FsStatus> {
    Ok(())
}

fn validate_package_id(package_id: &str) -> Result<(), FsStatus> {
    if package_id.is_empty() || package_id.len() > 128 {
        return Err(FsStatus::InvalidArgs);
    }
    if package_id
        .bytes()
        .any(|b| !b.is_ascii_alphanumeric() && b != b'.' && b != b'-' && b != b'_' && b != b':')
    {
        return Err(FsStatus::InvalidArgs);
    }
    Ok(())
}

fn validate_shared_vault_name(name: &str) -> Result<(), FsStatus> {
    if name.is_empty()
        || name.len() > 64
        || name == "."
        || name == ".."
        || name
            .bytes()
            .any(|b| !b.is_ascii_alphanumeric() && b != b'.' && b != b'-' && b != b'_')
    {
        return Err(FsStatus::InvalidArgs);
    }
    Ok(())
}

fn validate_domain(domain: &str) -> Result<(), FsStatus> {
    if domain.is_empty()
        || domain.len() > 255
        || domain.starts_with('.')
        || domain.contains("..")
        || domain
            .bytes()
            .any(|b| !b.is_ascii_lowercase() && !b.is_ascii_digit() && b != b'.' && b != b'-')
    {
        return Err(FsStatus::InvalidArgs);
    }
    Ok(())
}

fn validate_version_label(version: &str) -> Result<(), FsStatus> {
    if version.is_empty() || version.len() > 64 {
        return Err(FsStatus::InvalidArgs);
    }
    if version
        .bytes()
        .any(|b| !b.is_ascii_alphanumeric() && b != b'.' && b != b'-' && b != b'_' && b != b'+')
    {
        return Err(FsStatus::InvalidArgs);
    }
    Ok(())
}
