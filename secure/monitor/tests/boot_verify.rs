use bexos_secure_monitor::boot_verify::{Error, verify};
fn main() {
    let paths: Vec<_> = std::env::args_os().skip(1).collect();
    assert_eq!(paths.len(), 6);
    let images: Vec<_> = paths
        .iter()
        .map(|path| std::fs::read(path).unwrap())
        .collect();
    let [
        kernel,
        bootfs,
        metadata,
        root,
        wrong_policy,
        zero_generation,
    ] = images.as_slice()
    else {
        unreachable!()
    };
    let verified = verify(metadata, root, kernel, bootfs).unwrap();
    assert_eq!(verified.generation, 2);
    assert_eq!(
        verify(wrong_policy, root, kernel, bootfs),
        Err(Error::Policy)
    );
    assert_eq!(
        verify(zero_generation, root, kernel, bootfs),
        Err(Error::Policy)
    );
    let mut changed = kernel.clone();
    changed[0] ^= 1;
    assert_eq!(verify(metadata, root, &changed, bootfs), Err(Error::Kernel));
    let mut changed = bootfs.clone();
    let last = changed.len() - 1;
    changed[last] ^= 1;
    assert_eq!(verify(metadata, root, kernel, &changed), Err(Error::Bootfs));
    let mut changed = root.clone();
    changed[8] ^= 1;
    assert_eq!(
        verify(metadata, &changed, kernel, bootfs),
        Err(Error::Metadata)
    );
    for index in [0, 32, 112, 256, metadata.len() - 1] {
        let mut changed = metadata.clone();
        changed[index] ^= 1;
        assert!(verify(&changed, root, kernel, bootfs).is_err());
    }
    let mut appended = kernel.clone();
    appended.push(0);
    assert_eq!(
        verify(metadata, root, &appended, bootfs),
        Err(Error::Kernel)
    );
    let mut appended = bootfs.clone();
    appended.push(0);
    assert_eq!(
        verify(metadata, root, kernel, &appended),
        Err(Error::Bootfs)
    );
    let mut appended = metadata.clone();
    appended.push(0);
    assert_eq!(
        verify(&appended, root, kernel, bootfs),
        Err(Error::Metadata)
    );
    assert_eq!(
        verify(&metadata[..metadata.len() - 1], root, kernel, bootfs),
        Err(Error::Metadata)
    );
    println!(
        "monitor boot verification: exact kernel, BootFS, policy and vbmeta authenticated; tampering rejected"
    );
}
