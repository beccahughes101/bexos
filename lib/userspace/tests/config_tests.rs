use bexos_userspace::config::{
    ConfigError, ConfigTable, ConfigType, MAX_CONFIG_SNAPSHOT_LEN, encode_config, encode_config_v2,
    schema_fingerprint,
};

#[test]
fn config_table_decodes_scalar_values() {
    let enabled = [1];
    let limit = 42u32.to_le_bytes();
    let generation = 99u64.to_le_bytes();
    let blob = encode_config(&[
        ("enabled", ConfigType::Bool, &enabled),
        ("limit", ConfigType::Uint32, &limit),
        ("generation", ConfigType::Uint64, &generation),
        ("channel", ConfigType::String, b"qemu"),
        ("seed", ConfigType::Bytes, b"abc"),
    ]);

    let table = ConfigTable::parse(&blob).expect("config should parse");

    assert_eq!(table.version(), 1);
    assert_eq!(table.schema_fingerprint(), 0);
    assert_eq!(table.generation(), 0);
    assert_eq!(table.get_bool("enabled"), Ok(true));
    assert_eq!(table.get_u32("limit"), Ok(42));
    assert_eq!(table.get_u64("generation"), Ok(99));
    assert_eq!(table.get_string("channel"), Ok("qemu"));
    assert_eq!(table.get_bytes("seed"), Ok(&b"abc"[..]));
    assert!(table.get_bool("channel").is_err());
    assert!(table.get_u32("missing").is_err());
}

#[test]
fn config_table_decodes_v2_metadata() {
    let limit = 42u32.to_le_bytes();
    let fingerprint = schema_fingerprint(&[("limit", ConfigType::Uint32, true, 0)]);
    let blob = encode_config_v2(fingerprint, 7, &[("limit", ConfigType::Uint32, &limit)]).unwrap();

    let table = ConfigTable::parse(&blob).expect("config v2 should parse");

    assert_eq!(table.version(), 2);
    assert_eq!(table.schema_fingerprint(), fingerprint);
    assert_eq!(table.generation(), 7);
    assert_eq!(table.get_u32("limit"), Ok(42));
}

#[test]
fn config_table_rejects_oversized_snapshots() {
    let blob = vec![0; MAX_CONFIG_SNAPSHOT_LEN + 1];

    assert_eq!(ConfigTable::parse(&blob), Err(ConfigError::TooLarge));
}

#[test]
fn config_table_rejects_truncated_payloads() {
    let value = 7u32.to_le_bytes();
    let mut blob = encode_config(&[("limit", ConfigType::Uint32, &value)]);
    blob.pop();

    assert!(ConfigTable::parse(&blob).is_err());
}
