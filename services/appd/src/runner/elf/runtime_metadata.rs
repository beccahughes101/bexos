use alloc::vec::Vec;
use bexos_userspace::dynamic_link::{EncodedSymbol, TlsModule, encode_linker_data_v3};

use super::{ElfError, RuntimeSymbol};

pub fn encode(
    symbols: &[RuntimeSymbol],
    constructors: &[u64],
    tls_template: &[u8],
    tls_mem_size: u64,
    tls_align: u64,
    modules: &[bexos_elf::tls::Module],
) -> Result<Vec<u8>, ElfError> {
    let encoded_symbols: Vec<_> = symbols
        .iter()
        .map(|symbol| EncodedSymbol {
            name: &symbol.name,
            address: symbol.address,
        })
        .collect();
    encode_linker_data_v3(
        &encoded_symbols,
        constructors,
        tls_template,
        tls_mem_size,
        tls_align,
        bexos_boot::ARCHITECTURE_ID,
        &modules
            .iter()
            .map(|m| TlsModule {
                thread_offset: m.thread_offset,
                mem_size: m.mem_size,
            })
            .collect::<Vec<_>>(),
    )
    .ok_or(ElfError::Overflow)
}
