//! A trusted length followed by Zstandard data, both linked into the runtime.
//! Keep cold ELF loading small without accepting code from external storage.
use ruzstd::io::Read;
use wasmtime::{Result, bail};

pub fn decode(packed: &[u8]) -> Result<Vec<u8>> {
    let Some((length, compressed)) = packed.split_at_checked(8) else {
        bail!("truncated embedded code length");
    };
    let length = u64::from_le_bytes(length.try_into().unwrap());
    if length == 0 || length > 32 * 1024 * 1024 {
        bail!("invalid embedded code length");
    }
    let mut decoder = ruzstd::decoding::StreamingDecoder::new(compressed)
        .map_err(|_| wasmtime::format_err!("invalid embedded code frame"))?;
    let mut code = vec![0; length as usize];
    // Bound the decoder's history allocation independently of the output size.
    for chunk in code.chunks_mut(128 * 1024) {
        decoder
            .read_exact(chunk)
            .map_err(|_| wasmtime::format_err!("truncated embedded code"))?;
    }
    if decoder
        .read(&mut [0; 1])
        .map_err(|_| wasmtime::format_err!("invalid embedded code trailer"))?
        != 0
    {
        bail!("embedded code exceeds its declared length");
    }
    Ok(code)
}

#[cfg(test)]
mod tests {
    #[test]
    fn rejects_short_and_excess_output() {
        let compressed = ruzstd::encoding::compress_to_vec(
            &b"trusted code"[..],
            ruzstd::encoding::CompressionLevel::Fastest,
        );
        for length in [0u64, 11, 12, 13, u64::MAX] {
            let mut packed = length.to_le_bytes().to_vec();
            packed.extend(&compressed);
            assert_eq!(super::decode(&packed).is_ok(), length == 12);
        }
        assert!(super::decode(&[0; 7]).is_err());
    }
}
