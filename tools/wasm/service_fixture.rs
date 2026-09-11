//! Embed Bazel-built child bytes and prototxt-derived options in the service fixture.
fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    assert_eq!(
        args.len(),
        5,
        "service-fixture TEMPLATE CHILD OPTIONS OUTPUT"
    );
    let mut source = std::fs::read_to_string(&args[1]).unwrap();
    for (name, path) in [("CHILD", &args[2]), ("OPTIONS", &args[3])] {
        let bytes = std::fs::read(path).unwrap();
        assert!(bytes.len() < 4096);
        let escaped: String = bytes.iter().map(|b| format!("\\{b:02x}")).collect();
        source = source.replace(&format!("{{{{{name}}}}}"), &escaped);
        source = source.replace(&format!("{{{{{name}_LEN}}}}"), &bytes.len().to_string());
    }
    std::fs::write(&args[4], wat::parse_str(source).unwrap()).unwrap();
}
