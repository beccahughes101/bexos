//! Bazel-invoked signing of architecture-specific secure runtime candidates.
use bexos_secure_firmware::{Architecture, Component, HEADER_BYTES, MAGIC};
use rsa::pkcs8::DecodePrivateKey;

fn target(arch: &str, component: &str) -> (Architecture, Component) {
    let arch = match arch {
        "aarch64" => Architecture::Aarch64,
        "x86_64" => Architecture::X86_64,
        _ => panic!("unsupported firmware architecture"),
    };
    let component = match component {
        "trusty" => Component::Trusty,
        "hypervisor" => Component::Hypervisor,
        _ => panic!("unsupported firmware component"),
    };
    assert!(
        component.descriptor(arch).is_some(),
        "component unavailable on this architecture"
    );
    (arch, component)
}
pub fn build(args: &[String]) {
    assert_eq!(
        args.len(),
        7,
        "--firmware PRIVATE OUTPUT PUBLIC_OUTPUT GENERATION ARCH COMPONENT ELF"
    );
    let (arch, component) = target(&args[4], &args[5]);
    let generation: u64 = args[3].parse().expect("firmware generation");
    assert!(generation > 0);
    let key = rsa::RsaPrivateKey::from_pkcs8_der(&super::decode_pem(
        &std::fs::read_to_string(&args[0]).expect("read signing key"),
    ))
    .expect("RSA signing key");
    let root = super::avb_public_key(&key);
    let image = std::fs::read(&args[6]).expect("read candidate ELF");
    assert!(image.len() <= bexos_secure_firmware::MAX_IMAGE_BYTES);
    let descriptor = super::hash_descriptor(component.descriptor(arch).unwrap(), &image);
    let metadata =
        super::sign_metadata(key, generation, component.rollback_location(), &descriptor);
    let mut bundle = Vec::with_capacity(HEADER_BYTES + metadata.len() + image.len());
    bundle.extend_from_slice(MAGIC);
    bundle.extend_from_slice(&(metadata.len() as u64).to_le_bytes());
    bundle.extend_from_slice(&(image.len() as u64).to_le_bytes());
    bundle.extend_from_slice(&[0; 8]);
    bundle.extend_from_slice(&metadata);
    bundle.extend_from_slice(&image);
    bexos_secure_firmware::verify(&bundle, &root, arch, component, 0, 0)
        .expect("validate signed candidate");
    std::fs::write(&args[1], bundle).expect("write candidate bundle");
    std::fs::write(&args[2], root).expect("write candidate root");
}
pub fn verify(args: &[String]) {
    assert_eq!(
        args.len(),
        6,
        "--verify-firmware BUNDLE PUBLIC_KEY ARCH COMPONENT ACTIVE_GENERATION DURABLE_FLOOR"
    );
    let (arch, component) = target(&args[2], &args[3]);
    let bundle = std::fs::read(&args[0]).expect("read candidate bundle");
    let root = std::fs::read(&args[1]).expect("read saved firmware root");
    bexos_secure_firmware::verify(
        &bundle,
        &root,
        arch,
        component,
        args[4].parse().expect("active generation"),
        args[5].parse().expect("durable floor"),
    )
    .expect("authenticate candidate bundle");
}
