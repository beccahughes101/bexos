//! Versioned startup metadata shared by both loader backends and userspace.
use alloc::vec::Vec;
const LINKER_MAGIC: &[u8; 8] = b"BXLINK02";
const LINKER_VERSION: u32 = 2;
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EncodedSymbol<'a> {
    pub name: &'a str,
    pub address: u64,
}

pub fn encode_linker_data(
    symbols: &[EncodedSymbol<'_>],
    constructors: &[u64],
    tls_template: &[u8],
    tls_mem_size: u64,
    tls_align: u64,
) -> Option<Vec<u8>> {
    if tls_template.len() as u64 > tls_mem_size || tls_align == 0 || !tls_align.is_power_of_two() {
        return None;
    }
    let symbol_count = u32::try_from(symbols.len()).ok()?;
    let constructor_count = u32::try_from(constructors.len()).ok()?;
    let template_len = u32::try_from(tls_template.len()).ok()?;
    let mem_size = u32::try_from(tls_mem_size).ok()?;
    let align = u32::try_from(tls_align).ok()?;
    let mut out = Vec::new();
    out.extend_from_slice(LINKER_MAGIC);
    out.extend_from_slice(&LINKER_VERSION.to_le_bytes());
    out.extend_from_slice(&symbol_count.to_le_bytes());
    out.extend_from_slice(&constructor_count.to_le_bytes());
    out.extend_from_slice(&template_len.to_le_bytes());
    out.extend_from_slice(&mem_size.to_le_bytes());
    out.extend_from_slice(&align.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    for symbol in symbols {
        let name = symbol.name.as_bytes();
        let len: u16 = name.len().try_into().ok()?;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&symbol.address.to_le_bytes());
        out.extend_from_slice(name);
    }
    for constructor in constructors {
        out.extend_from_slice(&constructor.to_le_bytes());
    }
    out.extend_from_slice(tls_template);
    Some(out)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TlsModule {
    pub thread_offset: i64,
    pub mem_size: u64,
}

pub fn encode_linker_data_v3(
    symbols: &[EncodedSymbol<'_>],
    constructors: &[u64],
    template: &[u8],
    mem_size: u64,
    align: u64,
    architecture: u64,
    modules: &[TlsModule],
) -> Option<Vec<u8>> {
    if modules.len() > 65 || !matches!(architecture, 1 | 2) {
        return None;
    }
    for module in modules {
        let start = module.thread_offset as i128;
        let end = start + module.mem_size as i128;
        let valid = if architecture == 1 {
            start >= 16 && end <= 16 + mem_size as i128
        } else {
            start >= -(mem_size as i128) && end <= 0
        };
        if module.mem_size != 0 && !valid {
            return None;
        }
    }
    let original = encode_linker_data(symbols, constructors, template, mem_size, align)?;
    let mut out = original[..36].to_vec();
    out[..8].copy_from_slice(b"BXLINK03");
    out[8..12].copy_from_slice(&3u32.to_le_bytes());
    out.extend_from_slice(&architecture.to_le_bytes());
    out.extend_from_slice(&(modules.len() as u64).to_le_bytes());
    for module in modules {
        out.extend_from_slice(&module.thread_offset.to_le_bytes());
        out.extend_from_slice(&module.mem_size.to_le_bytes());
    }
    out.extend_from_slice(&original[36..]);
    Some(out)
}
