use crate::*;

impl ParsedElf {
    pub fn parse(bytes: &[u8]) -> Result<Self, ElfError> {
        Self::parse_for(bytes, Machine::current_guest())
    }

    pub fn parse_for(bytes: &[u8], machine: Machine) -> Result<Self, ElfError> {
        let plan = LoadPlan::parse(bytes, machine).map_err(ElfError::from)?;
        Ok(Self {
            entry_vaddr: plan.entry_vaddr,
            mappings: plan
                .segments
                .into_iter()
                .map(|segment| ElfMapping {
                    file_offset: segment.file_offset,
                    file_size: segment.file_size,
                    vaddr: segment.vaddr,
                    rights: segment.rights,
                    bss: segment.zero_fill.map(|zero_fill| BssMapping {
                        vaddr: zero_fill.vaddr,
                        size_bytes: zero_fill.size_bytes,
                    }),
                })
                .collect(),
            tls: plan.tls.map(|tls| TlsSegment {
                file_offset: tls.file_offset,
                file_size: tls.file_size,
                mem_size: tls.mem_size,
                align: tls.align,
            }),
        })
    }
}

impl From<ElfLoadError> for ElfError {
    fn from(error: ElfLoadError) -> Self {
        match error {
            ElfLoadError::TooSmall => Self::TooSmall,
            ElfLoadError::BadMagic => Self::BadMagic,
            ElfLoadError::UnsupportedClass => Self::UnsupportedClass,
            ElfLoadError::UnsupportedEndian => Self::UnsupportedEndian,
            ElfLoadError::UnsupportedVersion => Self::UnsupportedVersion,
            ElfLoadError::UnsupportedType => Self::UnsupportedType,
            ElfLoadError::UnsupportedMachine => Self::UnsupportedMachine,
            ElfLoadError::TooManyProgramHeaders => Self::TooManyProgramHeaders,
            ElfLoadError::BadProgramHeaderSize => Self::BadProgramHeaderSize,
            ElfLoadError::ProgramHeadersOutOfBounds => Self::ProgramHeadersOutOfBounds,
            ElfLoadError::SegmentOutOfBounds => Self::SegmentOutOfBounds,
            ElfLoadError::InvalidLoadSegment => Self::InvalidLoadSegment,
            ElfLoadError::WriteExecuteSegment => Self::WriteExecuteSegment,
            ElfLoadError::UnsupportedBssLayout => Self::UnsupportedBssLayout,
            ElfLoadError::InvalidTlsSegment => Self::InvalidTlsSegment,
            ElfLoadError::TlsSegmentOutOfBounds => Self::TlsSegmentOutOfBounds,
            ElfLoadError::MissingExecutableSegment => Self::MissingExecutableSegment,
            ElfLoadError::EntryOutsideExecutableSegment => Self::EntryOutsideExecutableSegment,
            ElfLoadError::Overflow => Self::Overflow,
        }
    }
}
