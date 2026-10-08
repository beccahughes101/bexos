//! Deliberately small loader for the fixed BootFS `native_runner` image.
//!
//! This is the only ELF mapping code retained by appd.  It accepts one
//! statically linked image, rejects PT_DYNAMIC, and has no dependency resolver
//! or relocation path. Static TLS is initialized before the first thread starts.
use super::{
    CreatedProcess, KernelError, KernelHandle, KernelOps, LaunchError, LaunchResult, PackageImage,
};
use alloc::vec::Vec;
use bexos_elf::{ElfError, ParsedElf, align_up_to, arch::Machine, load::PAGE_SIZE, tls};
use bexos_kernel_core::loader::{RIGHTS_EXECUTE, RIGHTS_READ, RIGHTS_WRITE};

pub const PACKAGE: &str = "bexos.platform.native_runner";
pub const PATH: &str = "/pkg/bin/native_runner";
const STACK_SIZE: u64 = bexos_boot::USER_STACK_SIZE;
const STACK_TOP: u64 = bexos_boot::USER_STACK_TOP;
const TLS_LOAD_BASE: u64 = 0xbe00_0000;
const PT_DYNAMIC: u32 = 2;

pub fn launch<K: KernelOps>(
    kernel: &mut K,
    image: PackageImage<'_>,
) -> Result<LaunchResult, LaunchError> {
    reject_non_static(image.bytes)?;
    let parsed = ParsedElf::parse(image.bytes).map_err(LaunchError::Elf)?;
    let job = kernel
        .create_component_job(
            PACKAGE,
            1,
            PACKAGE,
            crate::HardwareAccessTier::None,
            false,
            1,
        )
        .map_err(|source| error("native_runner_job", source))?;
    let created = match kernel.create_process_in_job(job.job, PACKAGE) {
        Ok(created) => created,
        Err(source) => {
            let _ = kernel.terminate_job(job.job, -1);
            let _ = kernel.close_handle(job.job);
            return Err(error("native_runner_process", source));
        }
    };
    match map_and_start(kernel, image, &parsed, job.job, created) {
        Ok(result) => Ok(result),
        Err(failure) => {
            let _ = kernel.terminate_job(job.job, -1);
            for handle in [
                created.root_vmar,
                created.address_space,
                created.process,
                job.job,
            ] {
                let _ = kernel.close_handle(handle);
            }
            Err(failure)
        }
    }
}

fn map_and_start<K: KernelOps>(
    kernel: &mut K,
    image: PackageImage<'_>,
    parsed: &ParsedElf,
    job: KernelHandle,
    created: CreatedProcess,
) -> Result<LaunchResult, LaunchError> {
    let mut owned = Vec::new();
    let result = (|| -> Result<LaunchResult, LaunchError> {
        for mapping in &parsed.mappings {
            if mapping.file_size != 0 {
                let start = usize::try_from(mapping.file_offset)
                    .map_err(|_| LaunchError::InvalidNativeRunner)?;
                let len = usize::try_from(mapping.file_size)
                    .map_err(|_| LaunchError::InvalidNativeRunner)?;
                let bytes = image
                    .bytes
                    .get(
                        start
                            ..start
                                .checked_add(len)
                                .ok_or(LaunchError::InvalidNativeRunner)?,
                    )
                    .ok_or(LaunchError::InvalidNativeRunner)?;
                let vmo = kernel
                    .create_vmo_from_bytes(bytes)
                    .map_err(|source| error("native_runner_segment_vmo", source))?;
                owned.push(vmo);
                kernel
                    .map_in_vm_space(
                        created.address_space,
                        vmo,
                        0,
                        page_round(mapping.file_size)?,
                        mapping.vaddr,
                        mapping.rights,
                    )
                    .map_err(|source| error("native_runner_segment_map", source))?;
                kernel
                    .release_vmo(vmo)
                    .map_err(|source| error("native_runner_segment_release", source))?;
                owned.retain(|candidate| *candidate != vmo);
            }
            if let Some(bss) = mapping.bss {
                let vmo = kernel
                    .create_vmo(bss.size_bytes, 0)
                    .map_err(|source| error("native_runner_bss_vmo", source))?;
                owned.push(vmo);
                kernel
                    .map_in_vm_space(
                        created.address_space,
                        vmo,
                        0,
                        bss.size_bytes,
                        bss.vaddr,
                        mapping.rights & !RIGHTS_EXECUTE,
                    )
                    .map_err(|source| error("native_runner_bss_map", source))?;
                kernel
                    .release_vmo(vmo)
                    .map_err(|source| error("native_runner_bss_release", source))?;
                owned.retain(|candidate| *candidate != vmo);
            }
        }
        let thread_pointer_vaddr = if parsed.tls.is_some() {
            let machine = Machine::current_guest();
            let layout = tls::layout(machine, core::iter::once((image.bytes, parsed.tls)))
                .map_err(LaunchError::Elf)?;
            let tls_vaddr = align_up_to(TLS_LOAD_BASE, layout.align)
                .ok_or(LaunchError::Elf(ElfError::Overflow))?;
            let (tls_bytes, thread_pointer) =
                tls::initialize(&layout, machine, tls_vaddr).map_err(LaunchError::Elf)?;
            let tls_vmo = kernel
                .create_vmo_from_bytes(&tls_bytes)
                .map_err(|source| error("native_runner_tls_vmo", source))?;
            owned.push(tls_vmo);
            kernel
                .map_in_vm_space(
                    created.address_space,
                    tls_vmo,
                    0,
                    tls_bytes.len() as u64,
                    tls_vaddr,
                    RIGHTS_READ | RIGHTS_WRITE,
                )
                .map_err(|source| error("native_runner_tls_map", source))?;
            kernel
                .release_vmo(tls_vmo)
                .map_err(|source| error("native_runner_tls_release", source))?;
            owned.retain(|candidate| *candidate != tls_vmo);
            thread_pointer
        } else {
            0
        };
        let stack = kernel
            .create_vmo(STACK_SIZE, 0)
            .map_err(|source| error("native_runner_stack_vmo", source))?;
        owned.push(stack);
        kernel
            .map_in_vm_space(
                created.address_space,
                stack,
                0,
                STACK_SIZE,
                STACK_TOP - STACK_SIZE,
                RIGHTS_READ | RIGHTS_WRITE,
            )
            .map_err(|source| error("native_runner_stack_map", source))?;
        kernel
            .release_vmo(stack)
            .map_err(|source| error("native_runner_stack_release", source))?;
        owned.retain(|candidate| *candidate != stack);
        let host = kernel
            .create_channel()
            .map_err(|source| error("native_runner_host_channel", source))?;
        let thread = match kernel.start_thread_in_process(
            created.process,
            created.address_space,
            parsed.entry_vaddr,
            STACK_TOP,
            thread_pointer_vaddr,
            Some(host.remote),
        ) {
            Ok(thread) => thread,
            Err(source) => {
                let _ = kernel.close_handle(host.local);
                let _ = kernel.close_handle(host.remote);
                return Err(error("native_runner_start", source));
            }
        };
        if let Err(source) = kernel.close_handle(created.root_vmar) {
            let _ = kernel.close_handle(thread);
            let _ = kernel.close_handle(host.local);
            return Err(error("native_runner_root_vmar_close", source));
        }
        Ok(LaunchResult {
            job_handle: job,
            process_handle: created.process,
            address_space_handle: created.address_space,
            main_thread_handle: thread,
            service_manager_handle: host.local,
            controller_handle: KernelHandle::none(),
            events_handle: KernelHandle::none(),
            runtime_linker_data: None,
            native_host: None,
        })
    })();
    if result.is_err() {
        for handle in owned {
            let _ = kernel.release_vmo(handle);
        }
    }
    result
}

fn reject_non_static(bytes: &[u8]) -> Result<(), LaunchError> {
    if bytes.len() < 64 {
        return Err(LaunchError::InvalidNativeRunner);
    }
    let phoff = usize::try_from(u64::from_le_bytes(
        bytes[32..40]
            .try_into()
            .map_err(|_| LaunchError::InvalidNativeRunner)?,
    ))
    .map_err(|_| LaunchError::InvalidNativeRunner)?;
    let phentsize = usize::from(u16::from_le_bytes(
        bytes[54..56]
            .try_into()
            .map_err(|_| LaunchError::InvalidNativeRunner)?,
    ));
    let phnum = usize::from(u16::from_le_bytes(
        bytes[56..58]
            .try_into()
            .map_err(|_| LaunchError::InvalidNativeRunner)?,
    ));
    if phentsize < 56 || phnum > 32 {
        return Err(LaunchError::InvalidNativeRunner);
    }
    for index in 0..phnum {
        let offset = phoff
            .checked_add(
                index
                    .checked_mul(phentsize)
                    .ok_or(LaunchError::InvalidNativeRunner)?,
            )
            .ok_or(LaunchError::InvalidNativeRunner)?;
        let kind = u32::from_le_bytes(
            bytes
                .get(offset..offset + 4)
                .ok_or(LaunchError::InvalidNativeRunner)?
                .try_into()
                .map_err(|_| LaunchError::InvalidNativeRunner)?,
        );
        if kind == PT_DYNAMIC {
            return Err(LaunchError::InvalidNativeRunner);
        }
    }
    Ok(())
}

fn page_round(value: u64) -> Result<u64, LaunchError> {
    value
        .checked_add(PAGE_SIZE - 1)
        .map(|value| value & !(PAGE_SIZE - 1))
        .ok_or(LaunchError::InvalidNativeRunner)
}

fn error(operation: &'static str, source: KernelError) -> LaunchError {
    LaunchError::Kernel { operation, source }
}
