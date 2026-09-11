use bexos_userspace::command::CommandOptions;
#[test]
fn command_metadata_roundtrips_and_rejects_ambiguous_environment() {
    let options = CommandOptions {
        arguments: vec!["tool".into(), "a b".into(), "".into()],
        environment: vec![
            ("HOME".into(), "/data".into()),
            ("A".into(), "one=two".into()),
        ],
    };
    let bytes = options.encode().unwrap();
    assert_eq!(CommandOptions::decode(&bytes).unwrap(), options);
    for n in 0..bytes.len() {
        assert!(CommandOptions::decode(&bytes[..n]).is_err());
    }
    let mut bad = options.clone();
    bad.environment.push(("A".into(), "other".into()));
    assert!(bad.encode().is_err());
    let mut bad = options.clone();
    bad.arguments.push("x\0y".into());
    assert!(bad.encode().is_err());
    let mut bad = options;
    bad.arguments = vec!["x".repeat(1024); 64];
    assert!(bad.encode().is_err());
}
