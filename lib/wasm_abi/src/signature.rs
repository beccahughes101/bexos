//! Bounded decoder for bexos.app.WasmChildSignature. Identity labels are inputs
//! to trust validation, never proof of trust themselves.
use crate::{
    Error,
    wire::{self, Reader},
};
use alloc::{string::String, vec::Vec};
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SignatureEnvelope {
    pub package_id: String,
    pub certificates: Vec<Vec<u8>>,
    pub signature: Vec<u8>,
    pub algorithm: u64,
}
impl SignatureEnvelope {
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > 16384 {
            return Err(Error::LimitExceeded);
        }
        let mut value = Self {
            package_id: String::new(),
            certificates: Vec::new(),
            signature: Vec::new(),
            algorithm: 1,
        };
        let mut seen = 0u8;
        let mut reader = Reader(bytes);
        while let Some((key, field)) = reader.field()? {
            if matches!(key, 1 | 3 | 4) {
                let bit = 1 << key;
                if seen & bit != 0 {
                    return Err(Error::InvalidEncoding);
                }
                seen |= bit;
            }
            match key {
                1 => {
                    let bytes = field.bytes()?;
                    if bytes.is_empty() || bytes.len() > 128 || bytes.contains(&0) {
                        return Err(Error::InvalidEncoding);
                    }
                    value.package_id = core::str::from_utf8(bytes)
                        .map_err(|_| Error::InvalidEncoding)?
                        .into();
                }
                2 => {
                    let bytes = field.bytes()?;
                    if bytes.is_empty() || bytes.len() > 2048 || value.certificates.len() >= 4 {
                        return Err(Error::LimitExceeded);
                    }
                    value.certificates.push(bytes.to_vec());
                }
                3 => {
                    let bytes = field.bytes()?;
                    if bytes.is_empty() || bytes.len() > 512 {
                        return Err(Error::LimitExceeded);
                    }
                    value.signature = bytes.to_vec();
                }
                4 => value.algorithm = field.number()?,
                _ => {}
            }
        }
        if value.package_id.is_empty()
            || value.certificates.is_empty()
            || value.signature.is_empty()
            || !matches!(value.algorithm, 1 | 2)
        {
            return Err(Error::InvalidEncoding);
        }
        Ok(value)
    }
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        let mut bytes = Vec::new();
        wire::bytes(&mut bytes, 1, self.package_id.as_bytes());
        for cert in &self.certificates {
            wire::bytes(&mut bytes, 2, cert);
        }
        wire::bytes(&mut bytes, 3, &self.signature);
        wire::number(&mut bytes, 4, self.algorithm);
        Self::decode(&bytes)?;
        Ok(bytes)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    fn valid() -> SignatureEnvelope {
        SignatureEnvelope {
            package_id: "test.child".into(),
            certificates: vec![vec![1; 32]],
            signature: vec![2; 64],
            algorithm: 1,
        }
    }
    #[test]
    fn signature_wire_rejects_truncation_duplicates_and_oversize_fields() {
        let value = valid();
        let encoded = value.encode().unwrap();
        assert_eq!(SignatureEnvelope::decode(&encoded).unwrap(), value);
        for n in 0..encoded.len() - 2 {
            assert!(SignatureEnvelope::decode(&encoded[..n]).is_err());
        }
        let mut duplicate = encoded.clone();
        wire::bytes(&mut duplicate, 1, b"forged.label");
        assert!(SignatureEnvelope::decode(&duplicate).is_err());
        let mut invalid = valid();
        invalid.certificates = vec![vec![1; 2049]];
        assert!(invalid.encode().is_err());
        let mut invalid = valid();
        invalid.algorithm = 99;
        assert!(invalid.encode().is_err());
        assert!(SignatureEnvelope::decode(&vec![0; 16385]).is_err());
    }
}
