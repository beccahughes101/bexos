use super::ResolvedLibraryDependency;
use crate::{
    KernelHandle, Manifest, PackageImage, PackageImageError, PackageImageResolver, PackageLibrary,
    PackageLibraryDependency, PackageLibraryKind, ProcessRunnerOptions,
    manifest::LibraryExportKind,
};
use alloc::borrow::Cow;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use bexos_kernel_core::bootfs::Bootfs;
use bexos_userspace::{Channel, Memory, fs, log};
pub struct Image<'a> {
    package: String,
    path: String,
    export_name: String,
    soname: String,
    symbol_prefix: String,
    abi_version: u32,
    kind: PackageLibraryKind,
    direct_dependencies: Vec<PackageLibraryDependency>,
    bytes: Cow<'a, [u8]>,
    handle: u64,
    vmo_offset: u64,
    close_handle: bool,
}
pub struct Resolver<'a> {
    images: Vec<Image<'a>>,
    runtime: Option<RuntimeImage>,
}
impl<'a> Resolver<'a> {
    pub fn boot(boot: &Bootfs<'a>, boot_handle: u64, manifests: &[Manifest]) -> Self {
        let mut images = Vec::new();
        for manifest in manifests {
            for p in &manifest.processes {
                if p.wave.is_none() {
                    continue;
                }
                let package_path = executable_path(p).expect("boot runner options");
                let path = format!(
                    "/boot/pkg/{}/{}",
                    manifest.package_name,
                    package_path.strip_prefix("/pkg/").expect("package path")
                );
                log(&format!("appd: resolver executable indexed {path}\n"));
                let entry = boot.find(&path).unwrap().expect("boot executable");
                images.push(Image {
                    package: manifest.package_name.clone(),
                    path: package_path.to_string(),
                    export_name: String::new(),
                    soname: String::new(),
                    symbol_prefix: String::new(),
                    abi_version: 0,
                    kind: PackageLibraryKind::Native,
                    direct_dependencies: Vec::new(),
                    bytes: Cow::Borrowed(entry.bytes),
                    handle: boot_handle,
                    vmo_offset: entry.payload_offset,
                    close_handle: false,
                });
            }
            for dependency in &manifest.library_dependencies {
                push_boot_library(boot, boot_handle, manifests, dependency, &mut images);
            }
        }
        let runtime = boot
            .find("/boot/pkg/bexos.platform.wasm_runner/bin/wasm_runner")
            .ok()
            .flatten()
            .and_then(|f| RuntimeImage::new(f.bytes.to_vec()).ok());
        Self { images, runtime }
    }
    pub fn disk_with_dependencies(
        root: Channel,
        manifest: &Manifest,
        dependencies: &[ResolvedLibraryDependency],
        vfsd: Channel,
        registry: &bexos_app_registry::MemoryAppRegistry,
    ) -> Result<Self, fs_fidl::FsStatus> {
        let mut images = Vec::new();
        for p in &manifest.processes {
            let package_path = executable_path(p).ok_or(fs_fidl::FsStatus::InvalidArgs)?;
            let path = package_path
                .strip_prefix("/pkg/")
                .ok_or(fs_fidl::FsStatus::InvalidArgs)?;
            let file = fs::open(root, path, 1 | 4)?;
            let bytes = read_owned_image(file, 256 * 1024 * 1024)?;
            let handle = Memory::from_bytes(&bytes).map_err(|_| fs_fidl::FsStatus::NoSpace)?;
            images.push(Image {
                package: manifest.package_name.clone(),
                path: package_path.to_string(),
                export_name: String::new(),
                soname: String::new(),
                symbol_prefix: String::new(),
                abi_version: 0,
                kind: PackageLibraryKind::Native,
                direct_dependencies: Vec::new(),
                bytes: Cow::Owned(bytes),
                handle,
                vmo_offset: 0,
                close_handle: true,
            });
        }
        for dependency in dependencies {
            let path = dependency
                .export_path
                .strip_prefix("/pkg/")
                .unwrap_or(&dependency.export_path);
            let file = fs::open(dependency.directory, path, 1 | 4)?;
            let bytes = read_owned_image(file, 256 * 1024 * 1024)?;
            images.push(Image {
                package: dependency.package_name.clone(),
                path: package_path(path),
                export_name: dependency.export_name.clone(),
                soname: dependency.soname.clone(),
                symbol_prefix: dependency.symbol_prefix.clone(),
                abi_version: dependency.abi_version,
                kind: dependency.kind,
                direct_dependencies: dependency.direct_dependencies.clone(),
                bytes: Cow::Owned(bytes),
                handle: 0,
                vmo_offset: 0,
                close_handle: false,
            });
        }
        Ok(Self {
            images,
            runtime: runtime_image(vfsd, registry, manifest),
        })
    }
}

fn package_path(path: &str) -> String {
    if path.starts_with("/pkg/") {
        path.into()
    } else {
        format!("/pkg/{path}")
    }
}

fn read_backing_image(file: Channel) -> Result<(Vec<u8>, u64), fs_fidl::FsStatus> {
    read_backing_image_bounded(file, 256 * 1024 * 1024)
}

fn read_owned_image(file: Channel, maximum: u64) -> Result<Vec<u8>, fs_fidl::FsStatus> {
    let image = read_backing_image_bounded(file, maximum);
    let close_file = fs::close(file);
    let (bytes, source) = image?;
    let close_source = Memory::close(source).map_err(|_| fs_fidl::FsStatus::Io);
    close_file?;
    close_source?;
    Ok(bytes)
}

fn read_backing_image_bounded(
    file: Channel,
    maximum: u64,
) -> Result<(Vec<u8>, u64), fs_fidl::FsStatus> {
    let (source, size) = fs::backing(file)?;
    if size == 0 || size > maximum {
        let _ = Memory::close(source);
        return Err(fs_fidl::FsStatus::InvalidArgs);
    }
    let rounded = bexos_boot::page_round(size).ok_or(fs_fidl::FsStatus::InvalidArgs)?;
    let va = match Memory::map(source, rounded, 2) {
        Ok(va) => va,
        Err(_) => {
            let _ = Memory::close(source);
            return Err(fs_fidl::FsStatus::NoSpace);
        }
    };
    let len = usize::try_from(size).map_err(|_| fs_fidl::FsStatus::InvalidArgs)?;
    let mut bytes = Vec::with_capacity(len);
    if len != 0 && Memory::commit_range(bytes.as_mut_ptr() as u64, size).is_err() {
        let _ = Memory::unmap(va, rounded);
        let _ = Memory::close(source);
        return Err(fs_fidl::FsStatus::NoSpace);
    }
    unsafe {
        core::ptr::copy_nonoverlapping(va as *const u8, bytes.as_mut_ptr(), len);
        bytes.set_len(len);
    }
    if Memory::unmap(va, rounded).is_err() {
        let _ = Memory::close(source);
        return Err(fs_fidl::FsStatus::Io);
    }
    Ok((bytes, source))
}
impl PackageImageResolver for Resolver<'_> {
    fn wasm_runtime_digest(&self) -> [u8; 32] {
        self.runtime
            .as_ref()
            .map_or(*include_bytes!(env!("BEXOS_WASM_RUNNER_DIGEST")), |r| {
                r.digest
            })
    }
    fn resolve_wasm_runtime(&self) -> Result<PackageImage<'_>, PackageImageError> {
        self.runtime
            .as_ref()
            .map(RuntimeImage::image)
            .ok_or(PackageImageError::NotFound)
    }
    fn resolve_executable<'a>(
        &'a self,
        package: &str,
        path: &str,
    ) -> Result<PackageImage<'a>, PackageImageError> {
        let image = self
            .images
            .iter()
            .find(|i| i.package == package && i.path == path)
            .ok_or(PackageImageError::NotFound)?;
        Ok(PackageImage {
            bytes: &image.bytes,
            vmo: KernelHandle { raw: image.handle },
            vmo_offset: image.vmo_offset,
        })
    }

    fn resolve_library<'a>(
        &'a self,
        package: &str,
        abi_version: u32,
    ) -> Result<PackageLibrary<'a>, PackageImageError> {
        let image = self
            .images
            .iter()
            .find(|i| {
                i.package == package
                    && i.abi_version == abi_version
                    && i.kind == PackageLibraryKind::Native
            })
            .ok_or(PackageImageError::NotFound)?;
        Ok(PackageLibrary {
            package_name: &image.package,
            export_name: &image.export_name,
            soname: &image.soname,
            image: PackageImage {
                bytes: &image.bytes,
                vmo: KernelHandle { raw: image.handle },
                vmo_offset: image.vmo_offset,
            },
            symbol_prefix: &image.symbol_prefix,
            abi_version: image.abi_version,
            kind: image.kind,
            direct_dependencies: &image.direct_dependencies,
        })
    }

    fn resolve_wasm_component<'b>(
        &'b self,
        package: &str,
        export_name: &str,
        abi_version: u32,
    ) -> Result<PackageLibrary<'b>, PackageImageError> {
        let image = self
            .images
            .iter()
            .find(|i| {
                i.package == package
                    && i.export_name == export_name
                    && i.abi_version == abi_version
                    && i.kind == PackageLibraryKind::WasmComponent
            })
            .ok_or(PackageImageError::NotFound)?;
        Ok(PackageLibrary {
            package_name: &image.package,
            export_name: &image.export_name,
            soname: &image.soname,
            image: PackageImage {
                bytes: &image.bytes,
                vmo: KernelHandle { raw: image.handle },
                vmo_offset: image.vmo_offset,
            },
            symbol_prefix: &image.symbol_prefix,
            abi_version: image.abi_version,
            kind: image.kind,
            direct_dependencies: &image.direct_dependencies,
        })
    }
}
impl Drop for Image<'_> {
    fn drop(&mut self) {
        if self.close_handle && self.handle != 0 {
            let _ = Memory::close(self.handle);
        }
    }
}

fn selected_library_export(
    manifest: &Manifest,
    abi_version: u32,
) -> Option<&crate::manifest::LibraryExport> {
    manifest.library_exports.iter().find(|export| {
        export.abi_version == abi_version
            && export.kind == LibraryExportKind::Native
            && !export.path.is_empty()
            && !export.symbol_prefix.is_empty()
            && !export.soname.is_empty()
    })
}

fn selected_component_export(
    manifest: &Manifest,
    abi_version: u32,
) -> Option<&crate::manifest::LibraryExport> {
    manifest.library_exports.iter().find(|export| {
        export.abi_version == abi_version
            && export.kind == LibraryExportKind::WasmComponent
            && !export.path.is_empty()
    })
}

fn selected_dependency_export(
    manifest: &Manifest,
    abi_version: u32,
) -> Option<&crate::manifest::LibraryExport> {
    selected_library_export(manifest, abi_version)
        .or_else(|| selected_component_export(manifest, abi_version))
}

fn push_boot_library<'a>(
    boot: &Bootfs<'a>,
    boot_handle: u64,
    manifests: &[Manifest],
    dependency: &crate::manifest::LibraryDependency,
    images: &mut Vec<Image<'a>>,
) {
    let dependency_manifest = manifests
        .iter()
        .find(|manifest| manifest.package_name == dependency.package_name)
        .expect("boot library manifest");
    let export = selected_dependency_export(dependency_manifest, dependency.abi_version)
        .expect("boot library export");
    for child in &dependency_manifest.library_dependencies {
        push_boot_library(boot, boot_handle, manifests, child, images);
    }
    let path = export.path.clone();
    if images.iter().any(|image| {
        image.package == dependency.package_name
            && image.export_name == export.name
            && image.abi_version == export.abi_version
    }) {
        return;
    }
    let boot_path = format!(
        "/boot/pkg/{}/{}",
        dependency.package_name,
        path.strip_prefix("/pkg/").expect("library path")
    );
    log(&format!("appd: resolver library indexed {boot_path}\n"));
    let entry = boot.find(&boot_path).unwrap().expect("boot library");
    images.push(Image {
        package: dependency.package_name.clone(),
        path,
        export_name: export.name.clone(),
        soname: export.soname.clone(),
        symbol_prefix: export.symbol_prefix.clone(),
        abi_version: export.abi_version,
        kind: match export.kind {
            LibraryExportKind::Native => PackageLibraryKind::Native,
            LibraryExportKind::WasmComponent => PackageLibraryKind::WasmComponent,
            LibraryExportKind::Unspecified => PackageLibraryKind::Native,
        },
        direct_dependencies: boot_library_dependency_metadata(manifests, dependency_manifest),
        bytes: Cow::Borrowed(entry.bytes),
        handle: boot_handle,
        vmo_offset: entry.payload_offset,
        close_handle: false,
    });
}

fn boot_library_dependency_metadata(
    manifests: &[Manifest],
    manifest: &Manifest,
) -> Vec<PackageLibraryDependency> {
    manifest
        .library_dependencies
        .iter()
        .filter_map(|dependency| {
            let dependency_manifest = manifests
                .iter()
                .find(|manifest| manifest.package_name == dependency.package_name)?;
            let export = selected_dependency_export(dependency_manifest, dependency.abi_version)?;
            Some(PackageLibraryDependency {
                package_name: dependency.package_name.clone(),
                abi_version: export.abi_version,
                soname: export.soname.clone(),
            })
        })
        .collect()
}

fn executable_path(process: &crate::manifest::Process) -> Option<&str> {
    match &process.runner_options {
        Some(ProcessRunnerOptions::Elf(o)) => Some(&o.path),
        Some(ProcessRunnerOptions::Wasm(o)) => Some(&o.path),
        Some(ProcessRunnerOptions::Nix(o)) => Some(&o.path),
        _ => None,
    }
}

pub struct RuntimeImage {
    pub digest: [u8; 32],
    bytes: Vec<u8>,
    handle: u64,
}
impl RuntimeImage {
    pub fn from_archive(bytes: &[u8]) -> Result<Self, crate::PackageImageError> {
        let verified = crate::runner::runtime_archive::verify(bytes)?;
        let mut image =
            Self::new(verified.bytes).map_err(|_| crate::PackageImageError::NotFound)?;
        image.digest = verified.digest;
        Ok(image)
    }

    fn new(bytes: Vec<u8>) -> Result<Self, fs_fidl::FsStatus> {
        let handle = Memory::from_bytes(&bytes).map_err(|_| fs_fidl::FsStatus::NoSpace)?;
        Ok(Self {
            bytes,
            handle,
            digest: *include_bytes!(env!("BEXOS_WASM_RUNNER_DIGEST")),
        })
    }
    pub fn image(&self) -> PackageImage<'_> {
        PackageImage {
            bytes: &self.bytes,
            vmo: KernelHandle { raw: self.handle },
            vmo_offset: 0,
        }
    }
}
impl Drop for RuntimeImage {
    fn drop(&mut self) {
        let _ = Memory::close(self.handle);
    }
}
pub fn runtime_image(
    vfsd: Channel,
    registry: &bexos_app_registry::MemoryAppRegistry,
    manifest: &Manifest,
) -> Option<RuntimeImage> {
    resolve_runtime_image(vfsd, registry, manifest, true)
}
/// A replacement without a nested runner selects the platform default, exactly
/// as a later disk launch of that replacement archive will.
pub fn default_runtime_image(
    vfsd: Channel,
    registry: &bexos_app_registry::MemoryAppRegistry,
    manifest: &Manifest,
) -> Option<RuntimeImage> {
    resolve_runtime_image(vfsd, registry, manifest, false)
}
fn resolve_runtime_image(
    vfsd: Channel,
    registry: &bexos_app_registry::MemoryAppRegistry,
    manifest: &Manifest,
    allow_package_override: bool,
) -> Option<RuntimeImage> {
    if !manifest
        .processes
        .iter()
        .any(|p| matches!(p.runner_options, Some(ProcessRunnerOptions::Wasm(_))))
    {
        return None;
    }
    let result = (|| {
        if let Some(record) = allow_package_override
            .then(|| registry.record(&manifest.package_name).ok())
            .flatten()
        {
            let root = bexos_userspace::vfs::get_package_directory(vfsd, &record.archive_id())?;
            let file = fs::open(root, crate::runner::runtime_archive::ARCHIVE_PATH, 1 | 4);
            let _ = Memory::close(root.0);
            match file {
                Ok(file) => {
                    let result = read_backing_image_bounded(
                        file,
                        crate::runner::runtime_archive::MAX_RUNTIME_ARCHIVE_BYTES as u64,
                    );
                    let _ = fs::close(file);
                    let (bytes, source) = result?;
                    let _ = Memory::close(source);
                    return RuntimeImage::from_archive(&bytes)
                        .map_err(|_| fs_fidl::FsStatus::AccessDenied);
                }
                Err(fs_fidl::FsStatus::NotFound) => {}
                Err(error) => return Err(error),
            }
        }
        let record = registry
            .record(bexos_wasm_abi::RUNNER_PACKAGE)
            .map_err(|_| fs_fidl::FsStatus::NotFound)?;
        let root = bexos_userspace::vfs::get_package_directory(vfsd, &record.archive_id())?;
        let file = fs::open(root, "bin/wasm_runner", 1 | 4);
        let _ = Memory::close(root.0);
        let file = file?;
        let result = read_backing_image(file);
        let _ = fs::close(file);
        let (bytes, source) = result?;
        let _ = Memory::close(source);
        RuntimeImage::new(bytes)
    })();
    match result {
        Ok(image) => Some(image),
        Err(e) => {
            log(&format!("appd: trusted WASM runtime unavailable: {e:?}\n"));
            None
        }
    }
}
