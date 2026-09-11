use alloc::string::String;
use alloc::vec::Vec;

use crate::KernelHandle;

pub const SYSTEM_UID: u64 = 0;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NamespaceEntry {
    pub path: String,
    pub directory: KernelHandle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DependencyNamespaceEntry<'a> {
    pub package_name: &'a str,
    pub mount_alias: Option<&'a str>,
    pub directory: KernelHandle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SharedVaultNamespaceEntry<'a> {
    pub name: &'a str,
    pub directory: KernelHandle,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StartupNamespace {
    entries: Vec<NamespaceEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NamespaceError {
    EmptyPath,
    DuplicatePath(String),
    MissingDirectory(String),
    InvalidMountName(String),
}

impl StartupNamespace {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(
        &mut self,
        path: impl Into<String>,
        directory: KernelHandle,
    ) -> Result<(), NamespaceError> {
        let path = path.into();
        if path.is_empty() {
            return Err(NamespaceError::EmptyPath);
        }
        if directory.is_none() {
            return Err(NamespaceError::MissingDirectory(path));
        }
        if self.entries.iter().any(|entry| entry.path == path) {
            return Err(NamespaceError::DuplicatePath(path));
        }
        self.entries.push(NamespaceEntry { path, directory });
        Ok(())
    }

    pub fn handles(&self) -> Vec<u64> {
        self.entries
            .iter()
            .map(|entry| entry.directory.raw)
            .collect()
    }

    pub fn paths(&self) -> String {
        let mut paths = String::new();
        for (index, entry) in self.entries.iter().enumerate() {
            if index != 0 {
                paths.push(';');
            }
            paths.push_str(&entry.path);
        }
        paths
    }

    pub fn entries(&self) -> &[NamespaceEntry] {
        &self.entries
    }
}

pub fn app_storage_namespace(
    package_root: KernelHandle,
    data_root: KernelHandle,
    tmp_root: KernelHandle,
) -> Result<StartupNamespace, NamespaceError> {
    app_storage_namespace_with_dependencies(package_root, data_root, tmp_root, &[])
}

pub fn app_storage_namespace_with_dependencies(
    package_root: KernelHandle,
    data_root: KernelHandle,
    tmp_root: KernelHandle,
    dependencies: &[DependencyNamespaceEntry<'_>],
) -> Result<StartupNamespace, NamespaceError> {
    app_storage_namespace_with_shared_vaults_and_dependencies(
        package_root,
        data_root,
        tmp_root,
        &[],
        dependencies,
    )
}

pub fn app_storage_namespace_with_shared_vaults_and_dependencies(
    package_root: KernelHandle,
    data_root: KernelHandle,
    tmp_root: KernelHandle,
    shared_vaults: &[SharedVaultNamespaceEntry<'_>],
    dependencies: &[DependencyNamespaceEntry<'_>],
) -> Result<StartupNamespace, NamespaceError> {
    let mut namespace = StartupNamespace::new();
    namespace.push("/pkg", package_root)?;
    namespace.push("/data", data_root)?;
    namespace.push("/tmp", tmp_root)?;
    for vault in shared_vaults {
        validate_mount_name(vault.name)?;
        namespace.push(shared_vault_path(vault.name), vault.directory)?;
    }
    for dependency in dependencies {
        let name = dependency.mount_alias.unwrap_or(dependency.package_name);
        validate_mount_name(name)?;
        namespace.push(dependency_path(name), dependency.directory)?;
    }
    Ok(namespace)
}

fn shared_vault_path(name: &str) -> String {
    let mut path = String::from("/shared/");
    path.push_str(name);
    path
}

fn dependency_path(name: &str) -> String {
    let mut path = String::from("/deps/");
    path.push_str(name);
    path
}

fn validate_mount_name(name: &str) -> Result<(), NamespaceError> {
    if name.is_empty()
        || name.len() > 64
        || name == "."
        || name == ".."
        || name
            .bytes()
            .any(|b| !b.is_ascii_alphanumeric() && b != b'.' && b != b'-' && b != b'_')
    {
        return Err(NamespaceError::InvalidMountName(name.into()));
    }
    Ok(())
}
