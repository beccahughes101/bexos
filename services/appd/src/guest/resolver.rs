use super::ResolvedLibraryDependency;
use crate::{
    KernelHandle, Manifest, PackageDirectories, PackageDirectoryDependency, PackageImage,
    PackageImageError, PackageImageResolver, PackageLibrary, PackageLibraryDependency,
    PackageLibraryKind, ProcessRunnerOptions, manifest::LibraryExportKind,
};
use alloc::borrow::Cow;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use bexos_app_registry::InstallSource;
use bexos_kernel_core::bootfs::Bootfs;
use bexos_userspace::{Channel, Memory, fs, log};
use core::sync::atomic::{AtomicU64, Ordering};

static FIXED_NATIVE_RUNNER_HANDLE: AtomicU64 = AtomicU64::new(0);
static FIXED_NATIVE_RUNNER_LEN: AtomicU64 = AtomicU64::new(0);

pub fn install_fixed_native_runner(handle: u64, len: u64) -> Result<(), fs_fidl::FsStatus> {
    if handle == 0 || len == 0 || len > 64 * 1024 * 1024 {
        return Err(fs_fidl::FsStatus::InvalidArgs);
    }
    FIXED_NATIVE_RUNNER_LEN.store(len, Ordering::Release);
    FIXED_NATIVE_RUNNER_HANDLE.store(handle, Ordering::Release);
    Ok(())
}
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
    providers: Vec<ProviderImage>,
    component_package: String,
    package_root: Option<u64>,
    dependency_roots: Vec<PackageDirectoryDependency>,
}
struct ProviderImage {
    package: String,
    directory: Option<u64>,
}
impl Drop for ProviderImage {
    fn drop(&mut self) {
        if let Some(directory) = self.directory {
            let _ = Memory::close(directory);
        }
    }
}
impl<'a> Resolver<'a> {
    pub fn boot(
        boot: &Bootfs<'a>,
        boot_handle: u64,
        manifests: &[Manifest],
        policy: &crate::platform_config::RunnerPolicy,
    ) -> Self {
        let mut images = Vec::new();
        for manifest in manifests {
            for p in &manifest.processes {
                if p.wave.is_none() {
                    continue;
                }
                let package_path =
                    match policy.provider_for(&p.runner).map(|provider| provider.kind) {
                        Some(crate::platform_config::ComponentRunnerProviderKind::DirectElf) => {
                            executable_path(p)
                        }
                        Some(
                            crate::platform_config::ComponentRunnerProviderKind::ComponentRunner,
                        ) => component_payload_path(p),
                        _ => None,
                    }
                    .expect("boot runner options");
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
        let native_path = "/boot/pkg/bexos.platform.native_runner/bin/native_runner";
        let native = boot
            .find(native_path)
            .unwrap()
            .expect("fixed BootFS native_runner");
        images.push(Image {
            package: crate::runner::bootstrap::PACKAGE.into(),
            path: crate::runner::bootstrap::PATH.into(),
            export_name: String::new(),
            soname: String::new(),
            symbol_prefix: String::new(),
            abi_version: 0,
            kind: PackageLibraryKind::Native,
            direct_dependencies: Vec::new(),
            bytes: Cow::Borrowed(native.bytes),
            handle: boot_handle,
            vmo_offset: native.payload_offset,
            close_handle: false,
        });
        for registration in &policy.component_runner_providers {
            if registration.kind
                != crate::platform_config::ComponentRunnerProviderKind::ComponentRunner
                || !manifests
                    .iter()
                    .flat_map(|manifest| &manifest.processes)
                    .any(|process| process.runner == registration.runner_name)
            {
                continue;
            }
            let boot_path = format!(
                "/boot/pkg/{}/{}",
                registration.package_id,
                registration
                    .executable_path
                    .strip_prefix("/pkg/")
                    .unwrap_or(&registration.executable_path)
            );
            if let Some(file) = boot.find(&boot_path).ok().flatten() {
                images.push(Image {
                    package: registration.package_id.clone(),
                    path: registration.executable_path.clone(),
                    export_name: String::new(),
                    soname: String::new(),
                    symbol_prefix: String::new(),
                    abi_version: 0,
                    kind: PackageLibraryKind::Native,
                    direct_dependencies: Vec::new(),
                    bytes: Cow::Borrowed(file.bytes),
                    handle: boot_handle,
                    vmo_offset: file.payload_offset,
                    close_handle: false,
                });
            }
        }
        Self {
            images,
            providers: Vec::new(),
            component_package: String::new(),
            package_root: None,
            dependency_roots: Vec::new(),
        }
    }
    pub fn disk_with_dependencies(
        root: Channel,
        manifest: &Manifest,
        dependencies: &[ResolvedLibraryDependency],
        vfsd: Channel,
        registry: &bexos_app_registry::MemoryAppRegistry,
        container: Option<(&str, Channel)>,
        policy: &crate::platform_config::RunnerPolicy,
    ) -> Result<Self, fs_fidl::FsStatus> {
        let mut images = Vec::new();
        for p in &manifest.processes {
            if policy.provider_for(&p.runner).map(|provider| provider.kind)
                != Some(crate::platform_config::ComponentRunnerProviderKind::DirectElf)
            {
                continue;
            }
            let package_path = executable_path(p).ok_or(fs_fidl::FsStatus::InvalidArgs)?;
            let (source, path) = if container.is_some_and(|(name, _)| name == p.name) {
                let (_, source) = container.unwrap();
                (
                    source,
                    package_path
                        .strip_prefix('/')
                        .ok_or(fs_fidl::FsStatus::InvalidArgs)?,
                )
            } else {
                (
                    root,
                    package_path
                        .strip_prefix("/pkg/")
                        .ok_or(fs_fidl::FsStatus::InvalidArgs)?,
                )
            };
            let file = fs::open(source, path, 1 | 4)?;
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
            if dependency.kind != PackageLibraryKind::Native {
                continue;
            }
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
        let mut providers = Vec::new();
        for registration in &policy.component_runner_providers {
            if registration.kind
                != crate::platform_config::ComponentRunnerProviderKind::ComponentRunner
                || !manifest
                    .processes
                    .iter()
                    .any(|process| process.runner == registration.runner_name)
            {
                continue;
            }
            if let Ok(directory) = provider_runtime_directory(
                vfsd,
                registry,
                manifest,
                &registration.runner_name,
                &registration.package_id,
                registration
                    .executable_path
                    .strip_prefix("/pkg/")
                    .unwrap_or(&registration.executable_path),
                &registration.expected_signer,
            ) {
                providers.push(ProviderImage {
                    package: registration.package_id.clone(),
                    directory: Some(directory),
                });
            }
        }
        images.push(fixed_native_runner_image()?);
        Ok(Self {
            images,
            providers,
            component_package: manifest.package_name.clone(),
            package_root: Some(root.0),
            dependency_roots: dependencies
                .iter()
                .map(|dependency| PackageDirectoryDependency {
                    package_name: dependency.package_name.clone(),
                    mount_alias: dependency.mount_alias.clone().unwrap_or_default(),
                    export_name: dependency.export_name.clone(),
                    export_path: dependency.export_path.clone(),
                    symbol_prefix: dependency.symbol_prefix.clone(),
                    soname: dependency.soname.clone(),
                    abi_version: dependency.abi_version,
                    kind: dependency.kind,
                    direct_dependencies: dependency.direct_dependencies.clone(),
                    directory: KernelHandle {
                        raw: dependency.directory.0,
                    },
                })
                .collect(),
        })
    }
}

fn fixed_native_runner_image() -> Result<Image<'static>, fs_fidl::FsStatus> {
    let bytes = fixed_native_runner_bytes()?;
    Ok(Image {
        package: crate::runner::bootstrap::PACKAGE.into(),
        path: crate::runner::bootstrap::PATH.into(),
        export_name: String::new(),
        soname: String::new(),
        symbol_prefix: String::new(),
        abi_version: 0,
        kind: PackageLibraryKind::Native,
        direct_dependencies: Vec::new(),
        bytes: Cow::Owned(bytes),
        handle: 0,
        vmo_offset: 0,
        close_handle: false,
    })
}

pub(super) fn fixed_native_runner_bytes() -> Result<Vec<u8>, fs_fidl::FsStatus> {
    let handle = FIXED_NATIVE_RUNNER_HANDLE.load(Ordering::Acquire);
    let len = FIXED_NATIVE_RUNNER_LEN.load(Ordering::Acquire);
    if handle == 0 || len == 0 || len > 64 * 1024 * 1024 {
        return Err(fs_fidl::FsStatus::NotFound);
    }
    let rounded = bexos_boot::page_round(len).ok_or(fs_fidl::FsStatus::InvalidArgs)?;
    let address = Memory::map(handle, rounded, 2).map_err(|_| fs_fidl::FsStatus::NoSpace)?;
    let mut bytes = Vec::with_capacity(len as usize);
    unsafe {
        core::ptr::copy_nonoverlapping(address as *const u8, bytes.as_mut_ptr(), len as usize);
        bytes.set_len(len as usize);
    }
    Memory::unmap(address, rounded).map_err(|_| fs_fidl::FsStatus::Io)?;
    Ok(bytes)
}

fn package_path(path: &str) -> String {
    if path.starts_with("/pkg/") {
        path.into()
    } else {
        format!("/pkg/{path}")
    }
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
    fn supports_directory_payloads(&self) -> bool {
        self.package_root.is_some()
    }

    fn component_directories(
        &self,
        package: &str,
    ) -> Result<PackageDirectories, PackageImageError> {
        // This resolver owns the launched component's root. Registered runner
        // providers have separately authenticated images and must never be
        // resolved through the component's package directory.
        if package != self.component_package {
            let package_dir = self
                .providers
                .iter()
                .find(|provider| provider.package == package)
                .and_then(|provider| provider.directory)
                .map(|directory| {
                    Memory::duplicate(directory, 1 | 2 | 4 | 32)
                        .map(|raw| KernelHandle { raw })
                        .map_err(|_| PackageImageError::AccessDenied)
                })
                .transpose()?;
            return Ok(PackageDirectories {
                package_dir,
                dependencies: Vec::new(),
            });
        }
        let package_dir = match self.package_root {
            Some(root) => Some(KernelHandle {
                raw: Memory::duplicate(root, 1 | 2 | 4 | 32)
                    .map_err(|_| PackageImageError::AccessDenied)?,
            }),
            None => None,
        };
        let mut dependencies: Vec<PackageDirectoryDependency> = Vec::new();
        for dependency in &self.dependency_roots {
            let mut duplicated = dependency.clone();
            duplicated.directory = KernelHandle {
                raw: match Memory::duplicate(dependency.directory.raw, 1 | 2 | 4 | 32) {
                    Ok(handle) => handle,
                    Err(_) => {
                        if let Some(package_dir) = package_dir {
                            let _ = Memory::close(package_dir.raw);
                        }
                        for prior in dependencies {
                            let _ = Memory::close(prior.directory.raw);
                        }
                        return Err(PackageImageError::AccessDenied);
                    }
                },
            };
            dependencies.push(duplicated);
        }
        Ok(PackageDirectories {
            package_dir,
            dependencies,
        })
    }

    fn resolve_executable<'a>(
        &'a self,
        package: &str,
        path: &str,
    ) -> Result<PackageImage<'a>, PackageImageError> {
        let Some(image) = self
            .images
            .iter()
            .find(|i| i.package == package && i.path == path)
        else {
            log(&format!(
                "appd: resolver executable missing package={package} path={path}\n"
            ));
            return Err(PackageImageError::NotFound);
        };
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
        _ => None,
    }
}

fn component_payload_path(process: &crate::manifest::Process) -> Option<&str> {
    match &process.runner_options {
        Some(ProcessRunnerOptions::Wasm(options)) => Some(&options.path),
        _ => None,
    }
}

pub fn provider_runtime_directory(
    vfsd: Channel,
    registry: &bexos_app_registry::MemoryAppRegistry,
    manifest: &Manifest,
    runner: &str,
    package: &str,
    path: &str,
    expected_signer: &str,
) -> Result<u64, fs_fidl::FsStatus> {
    if !manifest
        .processes
        .iter()
        .any(|process| process.runner == runner)
    {
        log(&format!(
            "appd: registered component runner unused runner={runner} package={package}\n"
        ));
        return Err(fs_fidl::FsStatus::InvalidArgs);
    }
    let record = registry.record(package).map_err(|_| {
        log(&format!(
            "appd: registered component runner record missing runner={runner} package={package}\n"
        ));
        fs_fidl::FsStatus::NotFound
    })?;
    let signer = match record.install_source {
        // BootFS is authenticated as part of the verified product image and
        // has no independent archive signer. System-image archives do: retain
        // that identity so an out-of-band update must match the same provider
        // signer configured for the product.
        InstallSource::Bootfs => "bexos_official_platform_v1",
        InstallSource::SystemImage | InstallSource::Debugd | InstallSource::Oci => record
            .verified_signer
            .as_ref()
            .map_or("", |signer| signer.root_anchor_id.as_str()),
    };
    if signer != expected_signer {
        log(&format!(
            "appd: registered component runner signer mismatch runner={runner} package={package} expected={expected_signer} actual={signer}\n"
        ));
        return Err(fs_fidl::FsStatus::AccessDenied);
    }
    let root = match bexos_userspace::vfs::get_package_directory(vfsd, &record.archive_id()) {
        Ok(root) => root,
        Err(error) => {
            log(&format!(
                "appd: registered component runner unavailable runner={runner} error={error:?}\n"
            ));
            return Err(error);
        }
    };
    let result: Result<(), fs_fidl::FsStatus> = (|| {
        let file = fs::open(root, path, 1 | 4);
        let file = file?;
        fs::close(file)
    })();
    match result {
        Ok(()) => Ok(root.0),
        Err(error) => {
            let _ = Memory::close(root.0);
            log(&format!(
                "appd: registered component runner unavailable runner={runner} error={error:?}\n"
            ));
            Err(error)
        }
    }
}
