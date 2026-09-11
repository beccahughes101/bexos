mod firmware;
use rsa::pkcs1v15::SigningKey;
use rsa::pkcs8::DecodePrivateKey;
use rsa::signature::{SignatureEncoding, Signer};
use rsa::traits::PublicKeyParts;
use rsa::{BigUint, RsaPrivateKey};
use sha2::{Digest, Sha256};

const HEADER_BYTES: usize = bexos_avb::HEADER_BYTES;
const AUTH_BYTES: usize = 320;
const PUBLIC_KEY_BYTES: usize = 520;

fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.first().is_some_and(|arg| arg == "--firmware") {
        firmware::build(&args[1..]);
        return;
    }
    if args.first().is_some_and(|arg| arg == "--verify-firmware") {
        firmware::verify(&args[1..]);
        return;
    }
    if args.first().is_some_and(|arg| arg == "--verify") {
        verify(&args[1..]);
        return;
    }
    if args.first().is_some_and(|arg| arg == "--public-key") {
        assert_eq!(args.len(), 3, "vbmeta --public-key PRIVATE_KEY OUTPUT");
        let pem = std::fs::read_to_string(&args[1]).expect("read AVB development key");
        let key =
            RsaPrivateKey::from_pkcs8_der(&decode_pem(&pem)).expect("PKCS#8 RSA development key");
        assert_eq!(key.size(), 256, "AVB development key must be RSA-2048");
        std::fs::write(&args[2], avb_public_key(&key)).expect("write AVB public key");
        return;
    }
    assert!(
        args.len() >= 5,
        "vbmeta PRIVATE_KEY OUTPUT PUBLIC_KEY_OUTPUT ROLLBACK_INDEX NAME=IMAGE..."
    );
    let pem = std::fs::read_to_string(&args[0]).expect("read AVB development key");
    let key = RsaPrivateKey::from_pkcs8_der(&decode_pem(&pem)).expect("PKCS#8 RSA development key");
    assert_eq!(key.size(), 256, "AVB development key must be RSA-2048");
    let rollback_index = args[3].parse::<u64>().expect("rollback index");

    let public_key = avb_public_key(&key);
    std::fs::write(&args[2], &public_key).expect("write AVB public key");
    let mut descriptors = Vec::new();
    for value in &args[4..] {
        let (name, path) = value.split_once('=').expect("partition NAME=IMAGE");
        descriptors.extend_from_slice(&hash_descriptor(
            name.as_bytes(),
            &std::fs::read(path).expect("read partition image"),
        ));
    }
    std::fs::write(
        &args[1],
        sign_metadata(key, rollback_index, 0, &descriptors),
    )
    .expect("write vbmeta");
}

fn sign_metadata(
    key: RsaPrivateKey,
    rollback_index: u64,
    rollback_location: u32,
    descriptors: &[u8],
) -> Vec<u8> {
    let public_key = avb_public_key(&key);
    let aux_unpadded = PUBLIC_KEY_BYTES + descriptors.len();
    let aux_len = align(aux_unpadded, 64);
    let mut aux = vec![0u8; aux_len];
    aux[..PUBLIC_KEY_BYTES].copy_from_slice(&public_key);
    aux[PUBLIC_KEY_BYTES..aux_unpadded].copy_from_slice(&descriptors);

    let mut header = [0u8; HEADER_BYTES];
    header[..4].copy_from_slice(b"AVB0");
    put_u32(&mut header, 4, 1);
    put_u64(&mut header, 12, AUTH_BYTES as u64);
    put_u64(&mut header, 20, aux_len as u64);
    put_u32(&mut header, 28, bexos_avb::ALGORITHM_SHA256_RSA2048);
    put_u64(&mut header, 32, 0);
    put_u64(&mut header, 40, 32);
    put_u64(&mut header, 48, 32);
    put_u64(&mut header, 56, 256);
    put_u64(&mut header, 64, 0);
    put_u64(&mut header, 72, PUBLIC_KEY_BYTES as u64);
    put_u64(&mut header, 96, PUBLIC_KEY_BYTES as u64);
    put_u64(&mut header, 104, descriptors.len() as u64);
    put_u64(&mut header, 112, rollback_index);
    put_u32(&mut header, 124, rollback_location);
    let release = b"bexos-avb-1";
    header[128..128 + release.len()].copy_from_slice(release);

    let mut signed = Vec::with_capacity(HEADER_BYTES + aux.len());
    signed.extend_from_slice(&header);
    signed.extend_from_slice(&aux);
    let digest = Sha256::digest(&signed);
    let signature = SigningKey::<Sha256>::new(key).sign(&signed).to_vec();
    assert_eq!(signature.len(), 256);

    let mut output = Vec::with_capacity(HEADER_BYTES + AUTH_BYTES + aux.len());
    output.extend_from_slice(&header);
    output.extend_from_slice(&digest);
    output.extend_from_slice(&signature);
    output.resize(HEADER_BYTES + AUTH_BYTES, 0);
    output.extend_from_slice(&aux);
    output
}

fn verify(args: &[String]) {
    assert!(
        args.len() >= 3,
        "vbmeta --verify VBMETA PUBLIC_KEY NAME=IMAGE..."
    );
    let bytes = std::fs::read(&args[0]).expect("read vbmeta");
    let key = std::fs::read(&args[1]).expect("read AVB public key");
    let modulus = key.get(8..264).expect("AVB RSA-2048 public key");
    let verified = bexos_avb::verify_vbmeta(&bytes, modulus).expect("authenticate vbmeta");
    for value in &args[2..] {
        let (name, path) = value.split_once('=').expect("partition NAME=IMAGE");
        let descriptor = verified
            .hash_descriptor(name.as_bytes())
            .expect("hash descriptor");
        bexos_avb::verify_partition(descriptor, &std::fs::read(path).expect("read image"))
            .expect("partition digest");
    }
}

fn hash_descriptor(name: &[u8], image: &[u8]) -> Vec<u8> {
    let payload_len = name.len() + 32;
    let total = align(132 + payload_len, 8);
    let mut descriptor = vec![0u8; total];
    put_u64(&mut descriptor, 0, bexos_avb::HASH_DESCRIPTOR_TAG);
    put_u64(&mut descriptor, 8, (total - 16) as u64);
    put_u64(&mut descriptor, 16, image.len() as u64);
    descriptor[24..30].copy_from_slice(b"sha256");
    put_u32(&mut descriptor, 56, name.len() as u32);
    put_u32(&mut descriptor, 60, 0);
    put_u32(&mut descriptor, 64, 32);
    descriptor[132..132 + name.len()].copy_from_slice(name);
    descriptor[132 + name.len()..132 + payload_len].copy_from_slice(&Sha256::digest(image));
    descriptor
}

fn avb_public_key(key: &RsaPrivateKey) -> Vec<u8> {
    let modulus = left_pad(key.n().to_bytes_be(), 256);
    let n0 = u32::from_be_bytes(modulus[252..256].try_into().unwrap());
    let mut inverse = 1u32;
    for _ in 0..5 {
        inverse = inverse.wrapping_mul(2u32.wrapping_sub(n0.wrapping_mul(inverse)));
    }
    let rr = (BigUint::from(1u8) << 4096usize) % key.n();
    let rr = left_pad(rr.to_bytes_be(), 256);
    let mut output = Vec::with_capacity(PUBLIC_KEY_BYTES);
    output.extend_from_slice(&2048u32.to_be_bytes());
    output.extend_from_slice(&0u32.wrapping_sub(inverse).to_be_bytes());
    output.extend_from_slice(&modulus);
    output.extend_from_slice(&rr);
    output
}

fn left_pad(value: Vec<u8>, len: usize) -> Vec<u8> {
    assert!(value.len() <= len);
    let mut output = vec![0; len - value.len()];
    output.extend_from_slice(&value);
    output
}

fn put_u32(output: &mut [u8], offset: usize, value: u32) {
    output[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
}

fn put_u64(output: &mut [u8], offset: usize, value: u64) {
    output[offset..offset + 8].copy_from_slice(&value.to_be_bytes());
}

const fn align(value: usize, alignment: usize) -> usize {
    (value + alignment - 1) & !(alignment - 1)
}

fn decode_pem(pem: &str) -> Vec<u8> {
    let encoded = pem
        .lines()
        .filter(|line| !line.starts_with("-----"))
        .collect::<String>();
    let mut output = Vec::new();
    let mut accumulator = 0u32;
    let mut bits = 0u32;
    for byte in encoded.bytes().filter(|byte| *byte != b'=') {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => panic!("invalid PEM base64"),
        };
        accumulator = (accumulator << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push((accumulator >> bits) as u8);
            accumulator &= (1 << bits) - 1;
        }
    }
    output
}
