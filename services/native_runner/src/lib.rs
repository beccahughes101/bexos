use bexos_appd::{
    ComponentArchitecture, CreatedJob, CreatedProcess, ElfRunnerOptions, HardwareAccessTier,
    KernelFidlOps, KernelHandle, KernelOps, LaunchRequest, Manifest, PackageIdentity, PackageImage,
    PackageImageError, PackageImageResolver, PackageLibrary, PackageLibraryDependency,
    PackageLibraryKind, PackageTrustTier, PrecreatedKernel, Process, ProcessRunnerOptions,
};
use bexos_native_loader::ElfRunner;
use bexos_userspace::{Channel, KernelTransport, Memory, fs};
use component_runner_fidl::{
    FidlDecode, FidlEncode, HandleRef, NativeRunnerHostEventsOnPreparedRequest,
    NativeRunnerHostPrepareRequest, NativeRunnerProviderKind,
};
use kernel_fidl::{
    HandleRef as KernelHandleRef, Status, SystemPrivilegedGetDelegatedProcessStatusRequest,
    SystemPrivilegedPublicClient,
};

const MAX_PROVIDER_BYTES: u64 = 256 * 1024 * 1024;
const PT_DYNAMIC: u32 = 2;

#[derive(Debug, Eq, PartialEq)]
pub enum Error {
    Channel,
    Envelope,
    InvalidProvider,
    Read,
    Load,
}

struct ProviderImage {
    package: String,
    path: String,
    image: MappedImage,
    libraries: Vec<DependencyImage>,
}

struct DependencyImage {
    package: String,
    export_name: String,
    soname: String,
    symbol_prefix: String,
    abi_version: u32,
    direct_dependencies: Vec<PackageLibraryDependency>,
    image: MappedImage,
}

struct MappedImage {
    handle: u64,
    address: u64,
    len: usize,
    mapped_len: u64,
}

impl MappedImage {
    fn new(handle: u64, size: u64, maximum: u64) -> Result<Self, Error> {
        if handle == 0 || size == 0 || size > maximum {
            let _ = Memory::close(handle);
            return Err(Error::Read);
        }
        let mapped_len = match bexos_boot::page_round(size) {
            Some(mapped_len) => mapped_len,
            None => {
                let _ = Memory::close(handle);
                return Err(Error::Read);
            }
        };
        let address = match Memory::map(handle, mapped_len, 2) {
            Ok(address) => address,
            Err(_) => {
                let _ = Memory::close(handle);
                return Err(Error::Read);
            }
        };
        let len = match usize::try_from(size) {
            Ok(len) => len,
            Err(_) => {
                let _ = Memory::unmap(address, mapped_len);
                let _ = Memory::close(handle);
                return Err(Error::Read);
            }
        };
        Ok(Self {
            handle,
            address,
            len,
            mapped_len,
        })
    }

    fn bytes(&self) -> &[u8] {
        unsafe { core::slice::from_raw_parts(self.address as *const u8, self.len) }
    }
}

impl Drop for MappedImage {
    fn drop(&mut self) {
        let _ = Memory::unmap(self.address, self.mapped_len);
        let _ = Memory::close(self.handle);
    }
}

/// A bounded, read-only package view used during early boot. Normal launches
/// read through a directory capability; before the filesystem service exists,
/// appd supplies one immutable VMO snapshot. Both sources expose the same
/// package-relative read operation to the loader and transfer one immutable
/// VMO into the resulting image.
struct BootfsPackageDirectoryAdapter {
    source: PackageSource,
}

enum PackageSource {
    Directory(u64),
    ImmutableVmo { handle: u64, size: u64 },
    Consumed,
}

impl BootfsPackageDirectoryAdapter {
    fn from_directory(directory: u64) -> Result<Self, Error> {
        if directory == 0 {
            return Err(Error::Envelope);
        }
        Ok(Self {
            source: PackageSource::Directory(directory),
        })
    }

    fn from_immutable_vmo(handle: u64, size: u64) -> Result<Self, Error> {
        if handle == 0 || size == 0 || size > MAX_PROVIDER_BYTES {
            return Err(Error::Envelope);
        }
        Ok(Self {
            source: PackageSource::ImmutableVmo { handle, size },
        })
    }

    fn read(mut self, path: &str) -> Result<MappedImage, Error> {
        match core::mem::replace(&mut self.source, PackageSource::Consumed) {
            PackageSource::Directory(directory) => {
                let result = (|| {
                    let relative = package_relative_path(path)?;
                    let file =
                        fs::open(Channel(directory), relative, 1 | 4).map_err(|_| Error::Read)?;
                    let backing = fs::backing(file).map_err(|_| Error::Read);
                    let _ = fs::close(file);
                    let (handle, size) = backing?;
                    MappedImage::new(handle, size, MAX_PROVIDER_BYTES)
                })();
                let _ = Memory::close(directory);
                result
            }
            PackageSource::ImmutableVmo { handle, size } => {
                MappedImage::new(handle, size, MAX_PROVIDER_BYTES)
            }
            PackageSource::Consumed => Err(Error::Envelope),
        }
    }
}

impl Drop for BootfsPackageDirectoryAdapter {
    fn drop(&mut self) {
        match self.source {
            PackageSource::Directory(handle) | PackageSource::ImmutableVmo { handle, .. }
                if handle != 0 =>
            {
                let _ = Memory::close(handle);
            }
            PackageSource::Directory(_)
            | PackageSource::ImmutableVmo { .. }
            | PackageSource::Consumed => {}
        }
    }
}

impl PackageImageResolver for ProviderImage {
    fn resolve_executable<'a>(
        &'a self,
        package: &str,
        path: &str,
    ) -> Result<PackageImage<'a>, PackageImageError> {
        if package != self.package || path != self.path {
            return Err(PackageImageError::AccessDenied);
        }
        Ok(PackageImage {
            bytes: self.image.bytes(),
            vmo: KernelHandle {
                raw: self.image.handle,
            },
            vmo_offset: 0,
        })
    }

    fn resolve_library<'a>(
        &'a self,
        package: &str,
        abi_version: u32,
    ) -> Result<PackageLibrary<'a>, PackageImageError> {
        let image = self
            .libraries
            .iter()
            .find(|image| image.package == package && image.abi_version == abi_version)
            .ok_or(PackageImageError::NotFound)?;
        Ok(PackageLibrary {
            package_name: &image.package,
            export_name: &image.export_name,
            soname: &image.soname,
            image: PackageImage {
                bytes: image.image.bytes(),
                vmo: KernelHandle {
                    raw: image.image.handle,
                },
                vmo_offset: 0,
            },
            symbol_prefix: &image.symbol_prefix,
            abi_version: image.abi_version,
            kind: PackageLibraryKind::Native,
            direct_dependencies: &image.direct_dependencies,
        })
    }
}

pub fn run(host: Channel) -> Result<(), Error> {
    let message = host.recv_blocking().map_err(|_| Error::Channel)?;
    if message.bytes.len() < 8
        || u64::from_le_bytes(message.bytes[..8].try_into().map_err(|_| Error::Envelope)?) != 1
    {
        close_all(&message.handles);
        return Err(Error::Envelope);
    }
    let handles = message
        .handles
        .iter()
        .map(|raw| HandleRef { raw: *raw })
        .collect::<Vec<_>>();
    let prepare = NativeRunnerHostPrepareRequest::decode(&message.bytes[8..], &handles)
        .map_err(|_| Error::Envelope)?;
    let info = prepare.prepare_info;
    if !valid_package(&info.provider_package) || !valid_path(&info.provider_path) {
        close_all(&message.handles);
        return Err(Error::InvalidProvider);
    }
    let provider = match (info.provider_image.len(), info.provider_package_dir.len()) {
        (0, 1) => BootfsPackageDirectoryAdapter::from_directory(info.provider_package_dir[0].raw)?,
        (1, 0) => BootfsPackageDirectoryAdapter::from_immutable_vmo(
            info.provider_image[0].raw,
            info.provider_image_size,
        )?,
        _ => return Err(Error::Envelope),
    };
    let provider_image = provider.read(info.provider_path)?;
    let directory_dependencies =
        info.dependency_images.is_empty() && info.dependency_image_sizes_le.is_empty();
    if directory_dependencies && info.dependency_directories.len() != info.dependencies.len() {
        return Err(Error::Envelope);
    }
    if !directory_dependencies && !info.dependency_directories.is_empty() {
        return Err(Error::Envelope);
    }
    if !directory_dependencies
        && (info.dependency_images.len() != info.dependencies.len()
            || info.dependency_image_sizes_le.len()
                != info
                    .dependencies
                    .len()
                    .checked_mul(8)
                    .ok_or(Error::Envelope)?)
    {
        return Err(Error::Envelope);
    }
    let mut libraries = Vec::with_capacity(info.dependencies.len());
    for index in 0..info.dependencies.len() {
        let dependency = info.dependencies.get(index).map_err(|_| Error::Envelope)?;
        let adapter = if directory_dependencies {
            let directory = info
                .dependency_directories
                .get(index)
                .ok_or(Error::Envelope)?;
            BootfsPackageDirectoryAdapter::from_directory(directory.raw)?
        } else {
            let handle = info.dependency_images[index].raw;
            let size_offset = index.checked_mul(8).ok_or(Error::Envelope)?;
            let size = u64::from_le_bytes(
                info.dependency_image_sizes_le
                    .get(size_offset..size_offset + 8)
                    .ok_or(Error::Envelope)?
                    .try_into()
                    .map_err(|_| Error::Envelope)?,
            );
            BootfsPackageDirectoryAdapter::from_immutable_vmo(handle, size)?
        };
        let image = adapter.read(dependency.export_path)?;
        let mut direct_dependencies = Vec::new();
        for direct_index in 0..dependency.direct_dependencies.len() {
            let direct = dependency
                .direct_dependencies
                .get(direct_index)
                .map_err(|_| Error::Envelope)?;
            direct_dependencies.push(PackageLibraryDependency {
                package_name: direct.package_name.into(),
                abi_version: direct.abi_version,
                soname: direct.soname.into(),
            });
        }
        libraries.push(DependencyImage {
            package: dependency.package_name.into(),
            export_name: dependency.export_name.into(),
            soname: dependency.soname.into(),
            symbol_prefix: dependency.symbol_prefix.into(),
            abi_version: dependency.abi_version,
            direct_dependencies,
            image,
        });
    }
    let image = ProviderImage {
        package: info.provider_package.into(),
        path: info.provider_path.into(),
        image: provider_image,
        libraries,
    };
    if info.provider_kind == NativeRunnerProviderKind::ComponentRunner
        && (!image.libraries.is_empty() || validate_static_provider(image.image.bytes()).is_err())
    {
        return Err(Error::InvalidProvider);
    }
    let process = Process {
        name: "component".into(),
        runner: "elf".into(),
        runner_options: Some(ProcessRunnerOptions::Elf(ElfRunnerOptions {
            path: image.path.clone(),
            stack_size_bytes: 0,
        })),
        ..Process::default()
    };
    let library_dependencies = image
        .libraries
        .iter()
        .map(|library| bexos_appd::LibraryDependency {
            package_name: library.package.clone(),
            version_requirement: None,
            mount_alias: None,
            abi_version: library.abi_version,
        })
        .collect();
    let manifest = Manifest {
        architecture: ComponentArchitecture::current_guest(),
        package_name: image.package.clone(),
        processes: vec![process.clone()],
        library_dependencies,
        ..Manifest::default()
    };
    let request = LaunchRequest {
        manifest: &manifest,
        process: &process,
        trust_tier: PackageTrustTier::PlatformCore,
        identity: PackageIdentity {
            package_id: &image.package,
            signer: "bexos_official_platform_v1",
            trust_tier: PackageTrustTier::PlatformCore,
            is_driver: false,
        },
        runner_policy: None,
        hardware_access: HardwareAccessTier::None,
        realtime_scheduling: false,
        resource_group_id: 0,
    };
    let inspection_process = (info.provider_kind == NativeRunnerProviderKind::DirectElf)
        .then(|| Memory::duplicate(info.target_process.raw, 1 | 2).map_err(|_| Error::Load))
        .transpose()?;
    let mut kernel = KernelFidlOps::new(KernelTransport(1), KernelTransport(2), KernelTransport(4));
    let startup_arg =
        (info.provider_kind == NativeRunnerProviderKind::ComponentRunner).then_some(KernelHandle {
            raw: info.runner.raw,
        });
    let mut delegated = PrecreatedKernel::new(
        &mut kernel,
        CreatedJob {
            job: KernelHandle {
                raw: info.target_job.raw,
            },
        },
        CreatedProcess {
            process: KernelHandle {
                raw: info.target_process.raw,
            },
            address_space: KernelHandle {
                raw: info.target_address_space.raw,
            },
            root_vmar: KernelHandle {
                raw: info.target_root_vmar.raw,
            },
        },
        startup_arg,
    );
    let result = ElfRunner::default()
        .launch(&request, &mut delegated, &image)
        .map_err(|_| Error::Load)?;
    drop(delegated);
    send_prepared(
        prepare.events.raw,
        Status::Ok,
        result.main_thread_handle.raw,
        result
            .runtime_linker_data
            .map(|(handle, len)| (handle.raw, len)),
    )?;

    if info.provider_kind == NativeRunnerProviderKind::DirectElf {
        // The direct target is already mapped and blocked on its bootstrap
        // channel. The host owns the standard runner endpoint and relays the
        // versioned Startup exchange without interpreting application metadata.
        let inspection_process = inspection_process.ok_or(Error::Load)?;
        let relay = relay_direct_runner(
            Channel(info.runner.raw),
            Channel(result.service_manager_handle.raw),
            inspection_process,
            info.provider_path,
            &mut kernel,
            result.job_handle,
        );
        let _ = Memory::close(inspection_process);
        relay?;
    } else {
        bexos_userspace::wait_terminated(result.process_handle.raw, -1)
            .map_err(|_| Error::Channel)?;
    }
    Ok(())
}

/// Registered execution providers are independently packaged static binaries.
/// Native application ELF remains free to use the full dependency loader.
fn validate_static_provider(bytes: &[u8]) -> Result<(), Error> {
    if bytes.len() < 64
        || bytes.get(..4) != Some(b"\x7fELF")
        || bytes.get(4) != Some(&2)
        || bytes.get(5) != Some(&1)
    {
        return Err(Error::InvalidProvider);
    }
    let phoff = usize::try_from(u64::from_le_bytes(
        bytes[32..40]
            .try_into()
            .map_err(|_| Error::InvalidProvider)?,
    ))
    .map_err(|_| Error::InvalidProvider)?;
    let phentsize = usize::from(u16::from_le_bytes(
        bytes[54..56]
            .try_into()
            .map_err(|_| Error::InvalidProvider)?,
    ));
    let phnum = usize::from(u16::from_le_bytes(
        bytes[56..58]
            .try_into()
            .map_err(|_| Error::InvalidProvider)?,
    ));
    if phentsize < 56 || phnum > 32 {
        return Err(Error::InvalidProvider);
    }
    for index in 0..phnum {
        let offset = phoff
            .checked_add(index.checked_mul(phentsize).ok_or(Error::InvalidProvider)?)
            .ok_or(Error::InvalidProvider)?;
        let kind = u32::from_le_bytes(
            bytes
                .get(offset..offset + 4)
                .ok_or(Error::InvalidProvider)?
                .try_into()
                .map_err(|_| Error::InvalidProvider)?,
        );
        if kind == PT_DYNAMIC {
            return Err(Error::InvalidProvider);
        }
    }
    Ok(())
}

fn relay_direct_runner(
    runner: Channel,
    target: Channel,
    process: u64,
    expected_path: &str,
    kernel: &mut KernelFidlOps<KernelTransport, KernelTransport, KernelTransport>,
    component_job: KernelHandle,
) -> Result<(), Error> {
    let start = bexos_component_runner::receive_start(
        runner,
        "elf",
        bexos_component_runner::ELF_PROGRAM_TYPE_URL,
        false,
    )
    .map_err(|_| Error::Envelope)?;
    let path = decode_elf_program_path(&start.program).ok_or(Error::Envelope)?;
    if path != expected_path || !valid_path(path) {
        return Err(Error::InvalidProvider);
    }
    let startup = start.startup.recv_blocking().map_err(|_| Error::Channel)?;
    target
        .send(&startup.bytes, &startup.handles)
        .map_err(|_| Error::Channel)?;
    let ready = if expected_path == "/pkg/bin/teed" {
        // Appd confirms that the secure-monitor authority was installed after
        // Startup delivery and teed intentionally withholds readiness until it
        // receives that confirmation.
        loop {
            let mut progressed = false;
            match start.startup.try_recv() {
                Ok(message) => {
                    target
                        .send(&message.bytes, &message.handles)
                        .map_err(|_| Error::Channel)?;
                    progressed = true;
                }
                Err(Status::ErrTimedOut) => {}
                Err(_) => return Err(Error::Channel),
            }
            match target.try_recv() {
                Ok(message) => break message,
                Err(Status::ErrTimedOut) => {}
                Err(_) => return Err(Error::Channel),
            }
            if !progressed {
                bexos_userspace::wait_channels(&[start.startup, target], -1)
                    .map_err(|_| Error::Channel)?;
            }
        }
    } else {
        target.recv_blocking().map_err(|_| Error::Channel)?
    };
    start
        .startup
        .send(&ready.bytes, &ready.handles)
        .map_err(|_| Error::Channel)?;
    bexos_component_runner::ready().map_err(|_| Error::Channel)?;
    loop {
        let mut progressed = false;
        if let Some(exit_code) = delegated_process_exit(process)? {
            bexos_component_runner::stop(Status::Ok, i64::from(exit_code))
                .map_err(|_| Error::Channel)?;
            return Ok(());
        }
        match bexos_component_runner::poll_controller().map_err(|_| Error::Channel)? {
            Some(bexos_component_runner::ControllerAction::Stop) => {
                target
                    .send(bexos_userspace::startup::GRACEFUL_STOP_MESSAGE_V1, &[])
                    .map_err(|_| Error::Channel)?;
                progressed = true;
            }
            Some(bexos_component_runner::ControllerAction::Kill) => {
                kernel
                    .terminate_job(component_job, 137)
                    .map_err(|_| Error::Load)?;
                bexos_component_runner::stop(Status::Ok, 137).map_err(|_| Error::Channel)?;
                return Ok(());
            }
            Some(bexos_component_runner::ControllerAction::Signal(signal)) => {
                target
                    .send(&signal.to_le_bytes(), &[])
                    .map_err(|_| Error::Channel)?;
                progressed = true;
            }
            Some(bexos_component_runner::ControllerAction::Connect(connection)) => {
                let mut metadata = format!(
                    "{}|{}|{}|",
                    connection.service, connection.protocol, connection.capability
                );
                for (index, ordinal) in connection.method_ordinals.iter().enumerate() {
                    if index != 0 {
                        metadata.push(',');
                    }
                    metadata.push_str(&ordinal.to_string());
                }
                metadata.push('|');
                metadata.push_str(&connection.permission_values.join(","));
                metadata.push('|');
                metadata.push_str(&connection.caller_package);
                metadata.push('|');
                metadata.push_str(&connection.caller_uid.to_string());
                metadata.push('|');
                metadata.push_str(if connection.caller_foreground {
                    "fg"
                } else {
                    "bg"
                });
                target
                    .send(metadata.as_bytes(), &[connection.endpoint])
                    .map_err(|_| Error::Channel)?;
                progressed = true;
            }
            None => {}
        }
        match start.startup.try_recv() {
            Ok(message) => {
                target
                    .send(&message.bytes, &message.handles)
                    .map_err(|_| Error::Channel)?;
                progressed = true;
            }
            Err(Status::ErrTimedOut) => {}
            Err(_) => return Err(Error::Channel),
        }
        match target.try_recv() {
            Ok(message) => {
                start
                    .startup
                    .send(&message.bytes, &message.handles)
                    .map_err(|_| Error::Channel)?;
                progressed = true;
            }
            Err(Status::ErrTimedOut) => {}
            Err(_) => return Err(Error::Channel),
        }
        if !progressed {
            let controller = bexos_component_runner::controller_channel().ok_or(Error::Channel)?;
            bexos_userspace::wait_channels(&[controller, start.startup, target], -1)
                .map_err(|_| Error::Channel)?;
        }
    }
}

fn decode_elf_program_path(bytes: &[u8]) -> Option<&str> {
    fn varint(bytes: &[u8], cursor: &mut usize) -> Option<u64> {
        let mut value = 0u64;
        for shift in (0..=63).step_by(7) {
            let byte = *bytes.get(*cursor)?;
            *cursor += 1;
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Some(value);
            }
        }
        None
    }

    let mut cursor = 0usize;
    let mut path = None;
    while cursor < bytes.len() {
        let key = varint(bytes, &mut cursor)?;
        let field = key >> 3;
        match key & 7 {
            0 => {
                varint(bytes, &mut cursor)?;
            }
            2 => {
                let len = usize::try_from(varint(bytes, &mut cursor)?).ok()?;
                let end = cursor.checked_add(len)?;
                let value = bytes.get(cursor..end)?;
                cursor = end;
                if field == 1 {
                    if path.is_some() {
                        return None;
                    }
                    path = Some(core::str::from_utf8(value).ok()?);
                }
            }
            _ => return None,
        }
    }
    path
}

fn delegated_process_exit(process: u64) -> Result<Option<i32>, Error> {
    let mut client = SystemPrivilegedPublicClient::new(KernelTransport(4));
    let mut request_bytes = [0u8; 24];
    let mut response_bytes = [0u8; 32];
    let mut request_handles = [KernelHandleRef { raw: 0 }; 2];
    let mut response_handles = [KernelHandleRef { raw: 0 }; 1];
    let response = client
        .get_delegated_process_status(
            &SystemPrivilegedGetDelegatedProcessStatusRequest {
                process_handle: KernelHandleRef { raw: process },
            },
            &mut request_bytes,
            &mut request_handles,
            &mut response_bytes,
            &mut response_handles,
        )
        .map_err(|_| Error::Channel)?;
    if response.status != Status::Ok {
        return Err(Error::Channel);
    }
    Ok(response.exited.then_some(response.exit_code))
}

fn send_prepared(
    events: u64,
    status: Status,
    thread: u64,
    runtime_linker_data: Option<(u64, u64)>,
) -> Result<(), Error> {
    let mut bytes = [0u8; 64];
    bytes[..8].copy_from_slice(&1u64.to_le_bytes());
    let mut handles = [HandleRef { raw: 0 }; 2];
    let linker_data = runtime_linker_data
        .iter()
        .map(|(handle, _)| HandleRef { raw: *handle })
        .collect::<Vec<_>>();
    let encoded = NativeRunnerHostEventsOnPreparedRequest {
        status,
        main_thread: HandleRef { raw: thread },
        runtime_linker_data: &linker_data,
        runtime_linker_data_len: runtime_linker_data.map_or(0, |(_, len)| len),
    }
    .encode(&mut bytes[8..], &mut handles)
    .map_err(|_| Error::Envelope)?;
    let raw_handles = handles[..encoded.handles]
        .iter()
        .map(|handle| handle.raw)
        .collect::<Vec<_>>();
    Channel(events)
        .send(&bytes[..8 + encoded.bytes], &raw_handles)
        .map_err(|_| Error::Channel)
}

fn valid_package(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn valid_path(value: &str) -> bool {
    package_relative_path(value).is_ok()
}

fn package_relative_path(value: &str) -> Result<&str, Error> {
    let relative = value.strip_prefix("/pkg/").ok_or(Error::InvalidProvider)?;
    if relative.is_empty()
        || relative.len() > 256
        || relative.contains('\\')
        || relative
            .split('/')
            .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        return Err(Error::InvalidProvider);
    }
    Ok(relative)
}

fn close_all(handles: &[u64]) {
    for handle in handles {
        let _ = Memory::close(*handle);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_identity_is_normalized() {
        assert!(valid_package("bexos.platform.wasm_runner"));
        assert!(!valid_package("bexos/platform"));
        assert!(valid_path("/pkg/bin/runner"));
        assert!(!valid_path("/pkg/bin/../runner"));
        assert!(!valid_path("/pkg/bin//runner"));
        assert!(!valid_path("/pkg/bin\\runner"));
        assert!(!valid_path("bin/runner"));
    }

    #[test]
    fn bootfs_package_adapter_rejects_unbounded_or_invalid_sources() {
        assert!(matches!(
            BootfsPackageDirectoryAdapter::from_directory(0),
            Err(Error::Envelope)
        ));
        assert!(matches!(
            BootfsPackageDirectoryAdapter::from_immutable_vmo(1, MAX_PROVIDER_BYTES + 1),
            Err(Error::Envelope)
        ));
        assert_eq!(
            package_relative_path("/pkg/lib/../secret"),
            Err(Error::InvalidProvider)
        );
        assert_eq!(
            package_relative_path("/pkg/lib//provider.so"),
            Err(Error::InvalidProvider)
        );
    }

    #[test]
    fn component_runner_provider_rejects_dynamic_elf() {
        let mut image = vec![0u8; 120];
        image[..6].copy_from_slice(b"\x7fELF\x02\x01");
        image[32..40].copy_from_slice(&64u64.to_le_bytes());
        image[54..56].copy_from_slice(&56u16.to_le_bytes());
        image[56..58].copy_from_slice(&1u16.to_le_bytes());
        image[64..68].copy_from_slice(&PT_DYNAMIC.to_le_bytes());
        assert_eq!(
            validate_static_provider(&image),
            Err(Error::InvalidProvider)
        );

        image[64..68].copy_from_slice(&1u32.to_le_bytes());
        assert_eq!(validate_static_provider(&image), Ok(()));
    }

    #[test]
    fn direct_elf_program_metadata_is_bounded_and_unambiguous() {
        let path = b"/pkg/bin/app";
        let mut encoded = vec![0x0a, path.len() as u8];
        encoded.extend_from_slice(path);
        assert_eq!(decode_elf_program_path(&encoded), Some("/pkg/bin/app"));

        let mut duplicate = encoded.clone();
        duplicate.extend_from_slice(&encoded);
        assert_eq!(decode_elf_program_path(&duplicate), None);
        assert_eq!(decode_elf_program_path(&[0x0a, 0x80]), None);
    }
}
