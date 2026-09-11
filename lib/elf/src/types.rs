use alloc::{rc::Rc, string::String, vec::Vec};
use core::ops::Deref;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParsedElf {
    pub entry_vaddr: u64,
    pub mappings: Vec<ElfMapping>,
    pub tls: Option<TlsSegment>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ElfMapping {
    pub file_offset: u64,
    pub file_size: u64,
    pub vaddr: u64,
    pub rights: u32,
    pub bss: Option<BssMapping>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BssMapping {
    pub vaddr: u64,
    pub size_bytes: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TlsSegment {
    pub file_offset: u64,
    pub file_size: u64,
    pub mem_size: u64,
    pub align: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ElfError {
    TooSmall,
    BadMagic,
    UnsupportedClass,
    UnsupportedEndian,
    UnsupportedVersion,
    UnsupportedType,
    UnsupportedMachine,
    TooManyProgramHeaders,
    BadProgramHeaderSize,
    ProgramHeadersOutOfBounds,
    SegmentOutOfBounds,
    InvalidLoadSegment,
    WriteExecuteSegment,
    UnsupportedBssLayout,
    InvalidTlsSegment,
    TlsSegmentOutOfBounds,
    MissingExecutableSegment,
    EntryOutsideExecutableSegment,
    MissingDynamicSection,
    UnsupportedDynamicEntry,
    UnsupportedRelocation,
    MissingDynamicSymbolTable,
    MissingDynamicStringTable,
    MissingDynamicHash,
    DynamicEntryOutOfBounds,
    DynamicSymbolOutOfBounds,
    RelocationOutOfBounds,
    RelocationTargetOutOfBounds,
    MissingSoname,
    SonameMismatch,
    UndeclaredNeededLibrary,
    MissingLibraryDependency,
    LibraryDependencyCycle,
    TooManyLibraries,
    LibraryAddressCollision,
    UnsupportedSymbolVersion,
    UnresolvedStrongSymbol,
    TextRelocation,
    Overflow,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeSymbol {
    pub name: String,
    pub address: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadedLibrary<'a> {
    pub source_bytes: &'a [u8],
    pub image: Rc<LoadedLibraryImage>,
}

impl Deref for LoadedLibrary<'_> {
    type Target = LoadedLibraryImage;

    fn deref(&self) -> &Self::Target {
        &self.image
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadedLibraryImage {
    /// Relocated initial TLS data, including zero-initialized storage.
    pub tls_template: Vec<u8>,
    pub segments: Vec<LoadedLibrarySegment>,
    pub symbols: Vec<RuntimeSymbol>,
    pub scope_symbols: Vec<ScopeSymbol>,
    pub constructors: Vec<u64>,
    pub tls: Option<TlsSegment>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadedLibrarySegment {
    pub bytes: Vec<u8>,
    pub size_bytes: u64,
    pub vaddr: u64,
    pub rights: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TlsSymbol {
    pub module: u64,
    pub offset: u64,
    pub thread_offset: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScopeSymbol {
    pub name: String,
    pub address: u64,
    pub weak: bool,
    pub tls: Option<TlsSymbol>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DynamicInfo {
    pub rela: u64,
    pub rela_size: u64,
    pub rela_entry_size: u64,
    pub plt_rela: u64,
    pub plt_rela_size: u64,
    pub plt_rela_kind: u64,
    pub relr: u64,
    pub relr_size: u64,
    pub relr_entry_size: u64,
    pub symtab: u64,
    pub symbol_entry_size: u64,
    pub strtab: u64,
    pub string_size: u64,
    pub hash: u64,
    pub gnu_hash: u64,
    pub soname: u64,
    pub needed: Vec<u64>,
    pub init: u64,
    pub init_array: u64,
    pub init_array_size: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProgramLoad {
    pub file_offset: u64,
    pub file_size: u64,
    pub mem_size: u64,
    pub vaddr: u64,
    pub rights: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DynamicSymbol {
    pub name_offset: u32,
    pub info: u8,
    pub visibility: u8,
    pub section_index: u16,
    pub value: u64,
}
