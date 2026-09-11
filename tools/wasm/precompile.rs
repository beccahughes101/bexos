//! Build trusted, runtime-embedded Pulley code; never consume device caches.
use sha2::{Digest, Sha256};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 3 {
        return Err("usage: precompile INPUT_WASM OUTPUT_CODE OUTPUT_SHA256".into());
    }
    let bytes = std::fs::read(&args[0])?;
    let limits = bexos_wasm_abi::Limits::default();
    let engine = wasmtime::Engine::new(&bexos_wasm_engine::config(&limits)?)?;
    let code = engine.precompile_component(&bytes)?;
    if code.len() > 32 * 1024 * 1024 {
        return Err("embedded component exceeds its decoded size limit".into());
    }
    let mut packed = (code.len() as u64).to_le_bytes().to_vec();
    packed.extend(ruzstd::encoding::compress_to_vec(
        code.as_slice(),
        ruzstd::encoding::CompressionLevel::Fastest,
    ));
    eprintln!(
        "Pulley artifact: {} bytes, packed: {} bytes",
        code.len(),
        packed.len()
    );
    std::fs::write(&args[1], packed)?;
    std::fs::write(&args[2], Sha256::digest(&bytes))?;
    Ok(())
}
