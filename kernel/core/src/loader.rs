//! Package image providers. ELF parsing is shared with appd.
pub use bexos_elf::load::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PackageImageMetadata {
    pub immutable: bool,
    pub trusted: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PackageImage<'a> {
    pub bytes: &'a [u8],
    pub metadata: PackageImageMetadata,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PackageLoadError {
    NotFound,
    AccessDenied,
    InvalidPath,
}

pub trait PackageProvider {
    fn resolve_executable<'a>(
        &'a self,
        package_name: &str,
        path: &str,
    ) -> Result<PackageImage<'a>, PackageLoadError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EmbeddedPackageEntry {
    pub package_name: &'static str,
    pub path: &'static str,
    pub bytes: &'static [u8],
    pub metadata: PackageImageMetadata,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EmbeddedPackageProvider {
    entries: &'static [EmbeddedPackageEntry],
}

impl EmbeddedPackageProvider {
    pub const fn new(entries: &'static [EmbeddedPackageEntry]) -> Self {
        Self { entries }
    }
}

impl PackageProvider for EmbeddedPackageProvider {
    fn resolve_executable<'a>(
        &'a self,
        package_name: &str,
        path: &str,
    ) -> Result<PackageImage<'a>, PackageLoadError> {
        if path.is_empty() || !path.starts_with("/pkg/") {
            return Err(PackageLoadError::InvalidPath);
        }

        self.entries
            .iter()
            .find(|entry| entry.package_name == package_name && entry.path == path)
            .map(|entry| PackageImage {
                bytes: entry.bytes,
                metadata: entry.metadata,
            })
            .ok_or(PackageLoadError::NotFound)
    }
}
