use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum X509SignatureAlgorithm {
    Ed25519,
    EcdsaP256Sha256,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum X509PublicKey<'a> {
    Ed25519(&'a [u8]),
    EcdsaP256(&'a [u8]),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParsedCertificate<'a> {
    pub raw_der: &'a [u8],
    pub tbs_der: &'a [u8],
    pub signature_algorithm: X509SignatureAlgorithm,
    pub signature: &'a [u8],
    pub serial: Vec<u8>,
    pub issuer_der: &'a [u8],
    pub subject_der: &'a [u8],
    pub not_before: u64,
    pub not_after: u64,
    pub spki_der: &'a [u8],
    pub public_key: X509PublicKey<'a>,
    pub is_ca: bool,
    pub has_key_usage: bool,
    pub digital_signature: bool,
    pub key_cert_sign: bool,
    pub has_eku: bool,
    pub code_signing_eku: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum X509Error {
    Malformed,
    UnsupportedAlgorithm,
    InvalidValidity,
}

const OID_ED25519: &[u8] = &[0x2b, 0x65, 0x70];
const OID_EC_PUBLIC_KEY: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01];
const OID_PRIME256V1: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07];
const OID_ECDSA_SHA256: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02];
const OID_BASIC_CONSTRAINTS: &[u8] = &[0x55, 0x1d, 0x13];
const OID_KEY_USAGE: &[u8] = &[0x55, 0x1d, 0x0f];
const OID_EXTENDED_KEY_USAGE: &[u8] = &[0x55, 0x1d, 0x25];
const OID_CODE_SIGNING: &[u8] = &[0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x03, 0x03];

pub fn parse_certificate(bytes: &[u8]) -> Result<ParsedCertificate<'_>, X509Error> {
    let mut top = DerReader::new(bytes);
    let cert = top.tlv()?;
    if cert.tag != 0x30 || !top.done() || cert.full.len() != bytes.len() {
        return Err(X509Error::Malformed);
    }
    let mut cert_body = DerReader::new(cert.content);
    let tbs = cert_body.tlv()?;
    let signature_algorithm = parse_algorithm_identifier(cert_body.tlv()?.content)?;
    let signature = parse_bit_string(cert_body.tlv()?.content)?;
    if !cert_body.done() || tbs.tag != 0x30 {
        return Err(X509Error::Malformed);
    }

    let mut tbs_body = DerReader::new(tbs.content);
    if tbs_body.peek_tag() == Some(0xa0) {
        let _ = tbs_body.tlv()?;
    }
    let serial = tbs_body.tlv()?;
    if serial.tag != 0x02 || serial.content.is_empty() {
        return Err(X509Error::Malformed);
    }
    let _tbs_signature = tbs_body.tlv()?;
    let issuer = tbs_body.tlv()?;
    let validity = tbs_body.tlv()?;
    let subject = tbs_body.tlv()?;
    let spki = tbs_body.tlv()?;
    if issuer.tag != 0x30 || validity.tag != 0x30 || subject.tag != 0x30 || spki.tag != 0x30 {
        return Err(X509Error::Malformed);
    }
    let (not_before, not_after) = parse_validity(validity.content)?;
    let public_key = parse_spki(spki.content)?;

    let mut parsed = ParsedCertificate {
        raw_der: bytes,
        tbs_der: tbs.full,
        signature_algorithm,
        signature,
        serial: serial.content.to_vec(),
        issuer_der: issuer.full,
        subject_der: subject.full,
        not_before,
        not_after,
        spki_der: spki.full,
        public_key,
        is_ca: false,
        has_key_usage: false,
        digital_signature: false,
        key_cert_sign: false,
        has_eku: false,
        code_signing_eku: false,
    };

    while !tbs_body.done() {
        let field = tbs_body.tlv()?;
        if field.tag == 0xa3 {
            parse_extensions(field.content, &mut parsed)?;
        }
    }
    Ok(parsed)
}

pub fn verify_certificate_signature(
    cert: &ParsedCertificate<'_>,
    issuer: &ParsedCertificate<'_>,
) -> bool {
    verify_signature(
        issuer.public_key,
        cert.signature_algorithm,
        cert.tbs_der,
        cert.signature,
    )
}

pub fn verify_signature(
    public_key: X509PublicKey<'_>,
    algorithm: X509SignatureAlgorithm,
    message: &[u8],
    signature: &[u8],
) -> bool {
    match (public_key, algorithm) {
        (X509PublicKey::Ed25519(key), X509SignatureAlgorithm::Ed25519) => {
            super::verify_ed25519_message(key, message, signature)
        }
        (X509PublicKey::EcdsaP256(key), X509SignatureAlgorithm::EcdsaP256Sha256) => {
            use p256::ecdsa::signature::Verifier;
            let Ok(verifying_key) = p256::ecdsa::VerifyingKey::from_sec1_bytes(key) else {
                return false;
            };
            if let Ok(sig) = p256::ecdsa::Signature::from_der(signature) {
                return verifying_key.verify(message, &sig).is_ok();
            }
            let Ok(sig) = p256::ecdsa::Signature::from_slice(signature) else {
                return false;
            };
            verifying_key.verify(message, &sig).is_ok()
        }
        _ => false,
    }
}

fn parse_extensions<'a>(
    bytes: &'a [u8],
    cert: &mut ParsedCertificate<'a>,
) -> Result<(), X509Error> {
    let mut explicit = DerReader::new(bytes);
    let extensions = explicit.tlv()?;
    if extensions.tag != 0x30 || !explicit.done() {
        return Err(X509Error::Malformed);
    }
    let mut list = DerReader::new(extensions.content);
    while !list.done() {
        let ext = list.tlv()?;
        if ext.tag != 0x30 {
            return Err(X509Error::Malformed);
        }
        let mut body = DerReader::new(ext.content);
        let oid = body.tlv()?;
        if oid.tag != 0x06 {
            return Err(X509Error::Malformed);
        }
        if body.peek_tag() == Some(0x01) {
            let boolean = body.tlv()?;
            if boolean.content.len() != 1 {
                return Err(X509Error::Malformed);
            }
        }
        let value = body.tlv()?;
        if value.tag != 0x04 || !body.done() {
            return Err(X509Error::Malformed);
        }
        match oid.content {
            OID_BASIC_CONSTRAINTS => parse_basic_constraints(value.content, cert)?,
            OID_KEY_USAGE => parse_key_usage(value.content, cert)?,
            OID_EXTENDED_KEY_USAGE => parse_extended_key_usage(value.content, cert)?,
            _ => {}
        }
    }
    Ok(())
}

fn parse_basic_constraints(
    bytes: &[u8],
    cert: &mut ParsedCertificate<'_>,
) -> Result<(), X509Error> {
    let mut value = DerReader::new(bytes);
    let seq = value.tlv()?;
    if seq.tag != 0x30 || !value.done() {
        return Err(X509Error::Malformed);
    }
    let mut fields = DerReader::new(seq.content);
    if fields.peek_tag() == Some(0x01) {
        let ca = fields.tlv()?;
        if ca.content.len() != 1 {
            return Err(X509Error::Malformed);
        }
        cert.is_ca = ca.content[0] != 0;
    }
    Ok(())
}

fn parse_key_usage(bytes: &[u8], cert: &mut ParsedCertificate<'_>) -> Result<(), X509Error> {
    let mut value = DerReader::new(bytes);
    let bits = value.tlv()?;
    if bits.tag != 0x03 || bits.content.is_empty() || !value.done() {
        return Err(X509Error::Malformed);
    }
    cert.has_key_usage = true;
    let payload = &bits.content[1..];
    cert.digital_signature = payload.first().is_some_and(|b| b & 0x80 != 0);
    cert.key_cert_sign = payload.first().is_some_and(|b| b & 0x04 != 0);
    Ok(())
}

fn parse_extended_key_usage(
    bytes: &[u8],
    cert: &mut ParsedCertificate<'_>,
) -> Result<(), X509Error> {
    let mut value = DerReader::new(bytes);
    let seq = value.tlv()?;
    if seq.tag != 0x30 || !value.done() {
        return Err(X509Error::Malformed);
    }
    cert.has_eku = true;
    let mut oids = DerReader::new(seq.content);
    while !oids.done() {
        let oid = oids.tlv()?;
        if oid.tag != 0x06 {
            return Err(X509Error::Malformed);
        }
        if oid.content == OID_CODE_SIGNING {
            cert.code_signing_eku = true;
        }
    }
    Ok(())
}

fn parse_validity(bytes: &[u8]) -> Result<(u64, u64), X509Error> {
    let mut fields = DerReader::new(bytes);
    let not_before = fields.tlv()?;
    let not_after = fields.tlv()?;
    if !fields.done() {
        return Err(X509Error::Malformed);
    }
    Ok((parse_time(not_before)?, parse_time(not_after)?))
}

fn parse_spki(bytes: &[u8]) -> Result<X509PublicKey<'_>, X509Error> {
    let mut spki = DerReader::new(bytes);
    let algorithm = spki.tlv()?;
    let key = spki.tlv()?;
    if algorithm.tag != 0x30 || key.tag != 0x03 || !spki.done() {
        return Err(X509Error::Malformed);
    }
    let key_bytes = parse_bit_string(key.content)?;
    let mut alg = DerReader::new(algorithm.content);
    let oid = alg.tlv()?;
    if oid.tag != 0x06 {
        return Err(X509Error::Malformed);
    }
    match oid.content {
        OID_ED25519 if alg.done() && key_bytes.len() == 32 => Ok(X509PublicKey::Ed25519(key_bytes)),
        OID_EC_PUBLIC_KEY => {
            let params = alg.tlv()?;
            if !alg.done() || params.tag != 0x06 || params.content != OID_PRIME256V1 {
                return Err(X509Error::UnsupportedAlgorithm);
            }
            if key_bytes.len() == 65 && key_bytes[0] == 0x04 {
                Ok(X509PublicKey::EcdsaP256(key_bytes))
            } else {
                Err(X509Error::Malformed)
            }
        }
        _ => Err(X509Error::UnsupportedAlgorithm),
    }
}

fn parse_algorithm_identifier(bytes: &[u8]) -> Result<X509SignatureAlgorithm, X509Error> {
    let mut alg = DerReader::new(bytes);
    let oid = alg.tlv()?;
    if oid.tag != 0x06 {
        return Err(X509Error::Malformed);
    }
    match oid.content {
        OID_ED25519 => Ok(X509SignatureAlgorithm::Ed25519),
        OID_ECDSA_SHA256 => Ok(X509SignatureAlgorithm::EcdsaP256Sha256),
        _ => Err(X509Error::UnsupportedAlgorithm),
    }
}

fn parse_bit_string(bytes: &[u8]) -> Result<&[u8], X509Error> {
    if bytes.is_empty() || bytes[0] != 0 {
        return Err(X509Error::Malformed);
    }
    Ok(&bytes[1..])
}

fn parse_time(value: DerValue<'_>) -> Result<u64, X509Error> {
    let text = core::str::from_utf8(value.content).map_err(|_| X509Error::Malformed)?;
    let (year, rest) = match value.tag {
        0x17 if text.len() == 13 && text.ends_with('Z') => {
            let yy = dec(&text[0..2])?;
            (
                (if yy >= 50 { 1900 + yy } else { 2000 + yy }) as i32,
                &text[2..12],
            )
        }
        0x18 if text.len() == 15 && text.ends_with('Z') => (dec(&text[0..4])? as i32, &text[4..14]),
        _ => return Err(X509Error::Malformed),
    };
    let month = dec(&rest[0..2])?;
    let day = dec(&rest[2..4])?;
    let hour = dec(&rest[4..6])?;
    let minute = dec(&rest[6..8])?;
    let second = dec(&rest[8..10])?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return Err(X509Error::InvalidValidity);
    }
    let days = days_from_civil(year, month as i32, day as i32);
    if days < 0 {
        return Err(X509Error::InvalidValidity);
    }
    Ok(days as u64 * 86_400 + hour as u64 * 3_600 + minute as u64 * 60 + second as u64)
}

fn dec(s: &str) -> Result<u32, X509Error> {
    let mut out = 0u32;
    for b in s.bytes() {
        if !b.is_ascii_digit() {
            return Err(X509Error::Malformed);
        }
        out = out * 10 + u32::from(b - b'0');
    }
    Ok(out)
}

fn days_from_civil(year: i32, month: i32, day: i32) -> i64 {
    let year = year - i32::from(month <= 2);
    let era = (if year >= 0 { year } else { year - 399 }) / 400;
    let yoe = year - era * 400;
    let month_prime = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * month_prime + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    i64::from(era * 146097 + doe - 719468)
}

#[derive(Clone, Copy)]
struct DerValue<'a> {
    tag: u8,
    content: &'a [u8],
    full: &'a [u8],
}

struct DerReader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> DerReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn done(&self) -> bool {
        self.offset == self.bytes.len()
    }

    fn peek_tag(&self) -> Option<u8> {
        self.bytes.get(self.offset).copied()
    }

    fn tlv(&mut self) -> Result<DerValue<'a>, X509Error> {
        let start = self.offset;
        let tag = *self.bytes.get(self.offset).ok_or(X509Error::Malformed)?;
        self.offset += 1;
        let first_len = *self.bytes.get(self.offset).ok_or(X509Error::Malformed)?;
        self.offset += 1;
        let len = if first_len & 0x80 == 0 {
            usize::from(first_len)
        } else {
            let octets = usize::from(first_len & 0x7f);
            if octets == 0 || octets > 4 {
                return Err(X509Error::Malformed);
            }
            let mut len = 0usize;
            for _ in 0..octets {
                len = (len << 8)
                    | usize::from(*self.bytes.get(self.offset).ok_or(X509Error::Malformed)?);
                self.offset += 1;
            }
            len
        };
        let end = self.offset.checked_add(len).ok_or(X509Error::Malformed)?;
        if end > self.bytes.len() {
            return Err(X509Error::Malformed);
        }
        let content = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(DerValue {
            tag,
            content,
            full: &self.bytes[start..end],
        })
    }
}
