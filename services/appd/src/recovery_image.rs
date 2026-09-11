extern crate alloc;

use crate::{PackageLibraryDependency, PackageLibraryKind};
use alloc::string::String;
use alloc::vec::Vec;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DriverRecoveryImage {
    pub package_id: String,
    pub process_name: String,
    pub executable: CachedImageVmo,
    pub libraries: Vec<CachedLibraryVmo>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CachedImageVmo {
    pub handle: u64,
    pub len: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CachedLibraryVmo {
    pub package_id: String,
    pub export_name: String,
    pub soname: String,
    pub symbol_prefix: String,
    pub abi_version: u32,
    pub kind: PackageLibraryKind,
    pub direct_dependencies: Vec<PackageLibraryDependency>,
    pub image: CachedImageVmo,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DriverRecoveryImageCache {
    images: Vec<DriverRecoveryImage>,
}

impl DriverRecoveryImageCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn upsert(&mut self, image: DriverRecoveryImage) {
        if let Some(existing) = self.images.iter_mut().find(|existing| {
            existing.package_id == image.package_id && existing.process_name == image.process_name
        }) {
            *existing = image;
        } else {
            self.images.push(image);
        }
    }

    pub fn remove_package(&mut self, package_id: &str) {
        self.images.retain(|image| image.package_id != package_id);
    }

    pub fn get(&self, package_id: &str, process_name: &str) -> Option<&DriverRecoveryImage> {
        self.images
            .iter()
            .find(|image| image.package_id == package_id && image.process_name == process_name)
    }

    pub fn images(&self) -> &[DriverRecoveryImage] {
        &self.images
    }
}
