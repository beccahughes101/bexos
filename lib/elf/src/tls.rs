use crate::{ElfError, PAGE_SIZE, TlsSegment, align_up_to, arch::Machine, page_round};
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Module {
    pub id: u64,
    pub offset: u64,
    pub thread_offset: i64,
    pub mem_size: u64,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TlsLayout {
    pub template: Vec<u8>,
    pub mem_size: u64,
    pub align: u64,
    pub modules: Vec<Module>,
}

/// Modules are ordered executable first, then dependency-first libraries.
/// x86 places the executable immediately below TP as required by local-exec TLS.
pub fn layout<'a>(
    machine: Machine,
    images: impl IntoIterator<Item = (&'a [u8], Option<TlsSegment>)>,
) -> Result<TlsLayout, ElfError> {
    let images: Vec<_> = images.into_iter().collect();
    let mut layout = TlsLayout {
        align: 1,
        ..TlsLayout::default()
    };
    for (index, (bytes, tls)) in images.iter().enumerate() {
        let mut module = Module {
            id: index as u64 + 1,
            ..Module::default()
        };
        if let Some(tls) = tls {
            module.mem_size = tls.mem_size;
            if tls.file_size > tls.mem_size || tls.align == 0 || !tls.align.is_power_of_two() {
                return Err(ElfError::InvalidTlsSegment);
            }
            if tls
                .file_offset
                .checked_add(tls.file_size)
                .is_none_or(|end| end > bytes.len() as u64)
            {
                return Err(ElfError::TlsSegmentOutOfBounds);
            }
            layout.align = layout.align.max(tls.align);
            match machine {
                Machine::Aarch64 => {
                    // Variant I aligns each module relative to TP, including
                    // the 16-byte TCB preceding the aggregate template.
                    module.offset = align_up_to(
                        layout.mem_size.checked_add(16).ok_or(ElfError::Overflow)?,
                        tls.align,
                    )
                    .ok_or(ElfError::Overflow)?
                    .checked_sub(16)
                    .ok_or(ElfError::Overflow)?;
                    module.thread_offset =
                        i64::try_from(module.offset.checked_add(16).ok_or(ElfError::Overflow)?)
                            .map_err(|_| ElfError::Overflow)?;
                    layout.mem_size = module
                        .offset
                        .checked_add(tls.mem_size)
                        .ok_or(ElfError::Overflow)?;
                }
                Machine::X86_64 => {
                    layout.mem_size = align_up_to(
                        layout
                            .mem_size
                            .checked_add(tls.mem_size)
                            .ok_or(ElfError::Overflow)?,
                        tls.align,
                    )
                    .ok_or(ElfError::Overflow)?;
                    module.thread_offset =
                        -i64::try_from(layout.mem_size).map_err(|_| ElfError::Overflow)?;
                }
            }
        }
        layout.modules.push(module);
    }
    if layout.mem_size == 0 {
        return Ok(layout);
    }
    layout.align = layout.align.max(PAGE_SIZE);
    layout.mem_size = align_up_to(layout.mem_size, layout.align).ok_or(ElfError::Overflow)?;
    let size = usize::try_from(layout.mem_size).map_err(|_| ElfError::Overflow)?;
    layout
        .template
        .try_reserve_exact(size)
        .map_err(|_| ElfError::Overflow)?;
    layout.template.resize(size, 0);
    for ((bytes, tls), module) in images.iter().zip(&mut layout.modules) {
        if let Some(tls) = tls {
            if machine == Machine::X86_64 {
                module.offset = layout
                    .mem_size
                    .checked_add_signed(module.thread_offset)
                    .ok_or(ElfError::Overflow)?;
            }
            let start = module.offset as usize;
            let end = start
                .checked_add(tls.file_size as usize)
                .ok_or(ElfError::Overflow)?;
            layout
                .template
                .get_mut(start..end)
                .ok_or(ElfError::InvalidTlsSegment)?
                .copy_from_slice(
                    &bytes[tls.file_offset as usize..(tls.file_offset + tls.file_size) as usize],
                );
        }
    }
    Ok(layout)
}

pub fn aggregate<'a>(
    executable_bytes: &'a [u8],
    executable: Option<TlsSegment>,
    libraries: impl IntoIterator<Item = (&'a [u8], Option<TlsSegment>)>,
) -> Result<TlsLayout, ElfError> {
    layout(
        Machine::current_guest(),
        core::iter::once((executable_bytes, executable)).chain(libraries),
    )
}

/// Initialize a mapped TLS allocation, including the architecture's TCB.
pub fn initialize(
    layout: &TlsLayout,
    machine: Machine,
    base: u64,
) -> Result<(Vec<u8>, u64), ElfError> {
    if layout.align == 0
        || !layout.align.is_power_of_two()
        || base % layout.align != 0
        || layout.template.len() as u64 > layout.mem_size
    {
        return Err(ElfError::InvalidTlsSegment);
    }
    let size = page_round(layout.mem_size.checked_add(16).ok_or(ElfError::Overflow)?)
        .ok_or(ElfError::Overflow)?;
    let mut bytes = alloc::vec![0; usize::try_from(size).map_err(|_| ElfError::Overflow)?];
    let (offset, tp) = match machine {
        Machine::Aarch64 => (16, base),
        Machine::X86_64 => (
            0,
            base.checked_add(layout.mem_size)
                .ok_or(ElfError::Overflow)?,
        ),
    };
    bytes[offset..offset + layout.template.len()].copy_from_slice(&layout.template);
    if machine == Machine::X86_64 {
        bytes[layout.mem_size as usize..layout.mem_size as usize + 8]
            .copy_from_slice(&tp.to_le_bytes());
    }
    Ok((bytes, tp))
}

impl TlsLayout {
    /// Replace the original file template after ELF relocations have completed.
    pub fn install_relocated(
        &mut self,
        index: usize,
        image: &crate::LoadedLibraryImage,
    ) -> Result<(), ElfError> {
        let module = self.modules.get(index).ok_or(ElfError::InvalidTlsSegment)?;
        if image.tls_template.len() as u64 != module.mem_size {
            return Err(ElfError::InvalidTlsSegment);
        }
        let start = usize::try_from(module.offset).map_err(|_| ElfError::Overflow)?;
        let end = start
            .checked_add(image.tls_template.len())
            .ok_or(ElfError::Overflow)?;
        self.template
            .get_mut(start..end)
            .ok_or(ElfError::InvalidTlsSegment)?
            .copy_from_slice(&image.tls_template);
        Ok(())
    }
}
