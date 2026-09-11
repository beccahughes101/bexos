use bexos_trust_store::persistent::TrustStoreDb;
use std::fs;

#[path = "../src/tool.rs"]
mod trust_roots;

#[test]
fn writes_tls_root_store_from_directory() {
    let dir = std::env::temp_dir().join(format!("bexos-trust-roots-tls-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("root.der"), b"dev tls root").unwrap();
    let out = dir.join("tls.redb");
    trust_roots::run([
        "--kind".to_string(),
        "tls".to_string(),
        "--input-dir".to_string(),
        dir.display().to_string(),
        "--out".to_string(),
        out.display().to_string(),
    ])
    .unwrap();
    let db = TrustStoreDb::open(&out).unwrap();
    assert_eq!(db.list_tls_roots().unwrap().len(), 1);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn writes_app_root_store_from_metadata() {
    let dir = std::env::temp_dir().join(format!("bexos-trust-roots-app-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("root.prototxt"),
        r#"
anchor_id: "bexos-dev"
tier: TIER1_PLATFORM_APP
algorithm: "Ed25519"
public_key_hex: "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"
permitted_package_prefixes: "bexos.*"
valid_from: 0
valid_until: 0
is_hardware_anchored: true
"#,
    )
    .unwrap();
    let out = dir.join("app.redb");
    trust_roots::run([
        "--kind".to_string(),
        "app".to_string(),
        "--input-dir".to_string(),
        dir.display().to_string(),
        "--out".to_string(),
        out.display().to_string(),
    ])
    .unwrap();
    let db = TrustStoreDb::open(&out).unwrap();
    assert_eq!(db.list_app_roots().unwrap()[0].anchor_id, "bexos-dev");
    let _ = fs::remove_dir_all(&dir);
}
