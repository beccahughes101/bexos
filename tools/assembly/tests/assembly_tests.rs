use bexos_assembly::test_proto::{bytes_field, concat, message_field, string_field, varint_field};
use bexos_assembly::{
    CompileConfigInput, ProductInput, compile_config_blob, compile_config_blob_from_product,
    validate_product,
};

#[test]
fn valid_product_assembly_indexes_selected_bundle_packages() {
    let manifest = manifest_with_schema("bexos.platform.storage_verify", vec![]);
    let bundle = bundle(
        "core_platform",
        vec![package(
            "//services/storage_verify:storage_verify",
            "bexos.platform.storage_verify",
            2,
        )],
    );
    let product = product(
        "qemu_dev",
        "//device/virtual/qemu/base/aarch64",
        "//device/virtual/qemu/nongui:platform_config_bin",
        vec!["core_platform"],
        vec![],
    );

    let output = validate_product(ProductInput {
        product: &product,
        bundles: &[bundle],
        manifests: &[("//services/storage_verify:storage_verify".into(), manifest)],
    })
    .expect("product should validate");

    assert_eq!(output.product_name, "qemu_dev");
    assert_eq!(
        output.packages[0].package_id,
        "bexos.platform.storage_verify"
    );
    assert!(output.index_text().contains("placement=SYSTEM_IMAGE"));
}

#[test]
fn product_rejects_missing_manifest_labels_and_duplicate_package_ids() {
    let missing_manifest_bundle = bundle(
        "core_platform",
        vec![package("//pkg:a", "com.example.same", 1)],
    );
    let product = product(
        "qemu_dev",
        "//device/virtual/qemu/base/aarch64",
        ":pcfg",
        vec!["core_platform"],
        vec![],
    );

    let missing = validate_product(ProductInput {
        product: &product,
        bundles: &[missing_manifest_bundle.clone()],
        manifests: &[],
    })
    .expect_err("missing manifest should fail");
    assert!(missing.contains("missing manifest"));

    let duplicate_bundle = bundle(
        "core_platform",
        vec![
            package("//pkg:a", "com.example.same", 1),
            package("//pkg:b", "com.example.same", 1),
        ],
    );
    let duplicate = validate_product(ProductInput {
        product: &product,
        bundles: &[duplicate_bundle],
        manifests: &[
            (
                "//pkg:a".into(),
                manifest_with_schema("com.example.same", vec![]),
            ),
            (
                "//pkg:b".into(),
                manifest_with_schema("com.example.same", vec![]),
            ),
        ],
    })
    .expect_err("duplicate id should fail");
    assert!(duplicate.contains("duplicate package id"));
}

#[test]
fn product_validates_library_dependencies() {
    let app_manifest = manifest_with_dependency("bexos.service.keychaind", "bexos.lib.crypto", 1);
    let crypto_manifest = library_manifest("bexos.lib.crypto", 1);
    let bundle = bundle(
        "core_platform",
        vec![
            package(
                "//services/keychaind:keychaind_elf",
                "bexos.service.keychaind",
                1,
            ),
            package("//lib/crypto:crypto_archive", "bexos.lib.crypto", 2),
        ],
    );
    let product = product(
        "qemu_dev",
        "//device/virtual/qemu/base/aarch64",
        ":pcfg",
        vec!["core_platform"],
        vec![],
    );

    validate_product(ProductInput {
        product: &product,
        bundles: &[bundle],
        manifests: &[
            ("//services/keychaind:keychaind_elf".into(), app_manifest),
            ("//lib/crypto:crypto_archive".into(), crypto_manifest),
        ],
    })
    .expect("library dependency should validate");
}

#[test]
fn product_validates_wasm_component_library_dependencies() {
    let app_manifest = manifest_with_dependency("bexos.app.dioxus_demo", "com.bexos.lib.dioxus", 1);
    let dioxus_manifest = wasm_component_library_manifest("com.bexos.lib.dioxus", 1);
    let bundle = bundle(
        "core_platform",
        vec![
            package("//apps/dioxus_demo:dioxus_demo", "bexos.app.dioxus_demo", 2),
            package(
                "//apps/dioxus_shared:dioxus_shared",
                "com.bexos.lib.dioxus",
                2,
            ),
        ],
    );
    let product = product(
        "qemu_dev",
        "//device/virtual/qemu/base/aarch64",
        ":pcfg",
        vec!["core_platform"],
        vec![],
    );

    validate_product(ProductInput {
        product: &product,
        bundles: &[bundle],
        manifests: &[
            ("//apps/dioxus_demo:dioxus_demo".into(), app_manifest),
            ("//apps/dioxus_shared:dioxus_shared".into(), dioxus_manifest),
        ],
    })
    .expect("WASM component library dependency should validate");
}

#[test]
fn product_rejects_missing_and_non_library_dependencies() {
    let app_manifest = manifest_with_dependency("bexos.service.keychaind", "bexos.lib.crypto", 1);
    let product = product(
        "qemu_dev",
        "//device/virtual/qemu/base/aarch64",
        ":pcfg",
        vec!["core_platform"],
        vec![],
    );
    let missing_bundle = bundle(
        "core_platform",
        vec![package(
            "//services/keychaind:keychaind_elf",
            "bexos.service.keychaind",
            1,
        )],
    );

    let missing = validate_product(ProductInput {
        product: &product,
        bundles: &[missing_bundle],
        manifests: &[(
            "//services/keychaind:keychaind_elf".into(),
            app_manifest.clone(),
        )],
    })
    .expect_err("missing dependency should fail");
    assert!(missing.contains("depends on missing library"));

    let non_library_bundle = bundle(
        "core_platform",
        vec![
            package(
                "//services/keychaind:keychaind_elf",
                "bexos.service.keychaind",
                1,
            ),
            package("//services/debugd:debugd_elf", "bexos.lib.crypto", 1),
        ],
    );
    let non_library = validate_product(ProductInput {
        product: &product,
        bundles: &[non_library_bundle],
        manifests: &[
            ("//services/keychaind:keychaind_elf".into(), app_manifest),
            (
                "//services/debugd:debugd_elf".into(),
                manifest_with_schema("bexos.lib.crypto", vec![]),
            ),
        ],
    })
    .expect_err("non-library dependency should fail");
    assert!(non_library.contains("depends on non-library package"));
}

#[test]
fn product_rejects_missing_library_dependency_abi() {
    let app_manifest = manifest_with_dependency("bexos.service.keychaind", "bexos.lib.crypto", 0);
    let crypto_manifest = library_manifest("bexos.lib.crypto", 1);
    let bundle = bundle(
        "core_platform",
        vec![
            package(
                "//services/keychaind:keychaind_elf",
                "bexos.service.keychaind",
                1,
            ),
            package("//lib/crypto:crypto_archive", "bexos.lib.crypto", 2),
        ],
    );
    let product = product(
        "qemu_dev",
        "//device/virtual/qemu/base/aarch64",
        ":pcfg",
        vec!["core_platform"],
        vec![],
    );

    let err = validate_product(ProductInput {
        product: &product,
        bundles: &[bundle],
        manifests: &[
            ("//services/keychaind:keychaind_elf".into(), app_manifest),
            ("//lib/crypto:crypto_archive".into(), crypto_manifest),
        ],
    })
    .expect_err("missing abi should fail");
    assert!(err.contains("without abi_version"));
}

#[test]
fn product_rejects_library_dependency_abi_mismatch() {
    let app_manifest = manifest_with_dependency("bexos.service.keychaind", "bexos.lib.crypto", 2);
    let crypto_manifest = library_manifest("bexos.lib.crypto", 1);
    let bundle = bundle(
        "core_platform",
        vec![
            package(
                "//services/keychaind:keychaind_elf",
                "bexos.service.keychaind",
                1,
            ),
            package("//lib/crypto:crypto_archive", "bexos.lib.crypto", 2),
        ],
    );
    let product = product(
        "qemu_dev",
        "//device/virtual/qemu/base/aarch64",
        ":pcfg",
        vec!["core_platform"],
        vec![],
    );

    let err = validate_product(ProductInput {
        product: &product,
        bundles: &[bundle],
        manifests: &[
            ("//services/keychaind:keychaind_elf".into(), app_manifest),
            ("//lib/crypto:crypto_archive".into(), crypto_manifest),
        ],
    })
    .expect_err("abi mismatch should fail");
    assert!(err.contains("abi_version 2"));
}

#[test]
fn product_validates_required_consumed_services_and_capabilities() {
    let consumer_manifest = manifest_consuming_service(
        "com.example.client",
        "bexos.hardware.Camera",
        1,
        "Camera",
        2,
    );
    let provider_manifest = manifest_exposing_service(
        "bexos.hardware.camera",
        "bexos.hardware.Camera",
        1,
        "Camera",
        2,
    );
    let bundle = bundle(
        "core_platform",
        vec![
            package("//pkg:client", "com.example.client", 1),
            package("//pkg:camera", "bexos.hardware.camera", 1),
        ],
    );
    let product = product(
        "qemu_dev",
        "//device/virtual/qemu/base/aarch64",
        ":pcfg",
        vec!["core_platform"],
        vec![],
    );

    validate_product(ProductInput {
        product: &product,
        bundles: &[bundle],
        manifests: &[
            ("//pkg:client".into(), consumer_manifest),
            ("//pkg:camera".into(), provider_manifest),
        ],
    })
    .expect("required consumed service should validate");
}

#[test]
fn product_rejects_missing_required_consumed_service() {
    let consumer_manifest = manifest_consuming_service(
        "com.example.client",
        "bexos.hardware.Camera",
        1,
        "Camera",
        2,
    );
    let bundle = bundle(
        "core_platform",
        vec![package("//pkg:client", "com.example.client", 1)],
    );
    let product = product(
        "qemu_dev",
        "//device/virtual/qemu/base/aarch64",
        ":pcfg",
        vec!["core_platform"],
        vec![],
    );

    let err = validate_product(ProductInput {
        product: &product,
        bundles: &[bundle],
        manifests: &[("//pkg:client".into(), consumer_manifest)],
    })
    .expect_err("missing required service should fail");
    assert!(err.contains("depends on missing service"));
}

#[test]
fn product_allows_missing_optional_consumed_service() {
    let consumer_manifest = manifest_consuming_service(
        "com.example.client",
        "bexos.hardware.Camera",
        2,
        "Camera",
        2,
    );
    let bundle = bundle(
        "core_platform",
        vec![package("//pkg:client", "com.example.client", 1)],
    );
    let product = product(
        "qemu_dev",
        "//device/virtual/qemu/base/aarch64",
        ":pcfg",
        vec!["core_platform"],
        vec![],
    );

    validate_product(ProductInput {
        product: &product,
        bundles: &[bundle],
        manifests: &[("//pkg:client".into(), consumer_manifest)],
    })
    .expect("missing optional service should validate");
}

#[test]
fn product_rejects_unknown_consumed_capability_ordinal() {
    let consumer_manifest = manifest_consuming_service(
        "com.example.client",
        "bexos.hardware.Camera",
        1,
        "Camera",
        99,
    );
    let provider_manifest = manifest_exposing_service(
        "bexos.hardware.camera",
        "bexos.hardware.Camera",
        1,
        "Camera",
        2,
    );
    let bundle = bundle(
        "core_platform",
        vec![
            package("//pkg:client", "com.example.client", 1),
            package("//pkg:camera", "bexos.hardware.camera", 1),
        ],
    );
    let product = product(
        "qemu_dev",
        "//device/virtual/qemu/base/aarch64",
        ":pcfg",
        vec!["core_platform"],
        vec![],
    );

    let err = validate_product(ProductInput {
        product: &product,
        bundles: &[bundle],
        manifests: &[
            ("//pkg:client".into(), consumer_manifest),
            ("//pkg:camera".into(), provider_manifest),
        ],
    })
    .expect_err("unknown ordinal should fail");
    assert!(err.contains("unknown ordinal"));
}

#[test]
fn config_compiler_fills_defaults_and_overrides_scalars() {
    let manifest = manifest_with_schema(
        "com.example.configured",
        vec![
            field("enabled", 1, true, Some(value_bool(false)), 0),
            field("limit", 2, true, Some(value_u32(7)), 0),
            field("channel", 4, true, Some(value_string("stable")), 16),
        ],
    );
    let override_bytes = override_values(
        "com.example.configured",
        vec![
            assignment("enabled", value_bool(true)),
            assignment("limit", value_u32(42)),
        ],
    );

    let blob = compile_config_blob(CompileConfigInput {
        manifest: &manifest,
        override_bytes: Some(&override_bytes),
    })
    .expect("config should compile");

    assert_eq!(&blob[..8], b"BEXCFG\0\0");
    assert_eq!(u32::from_le_bytes(blob[8..12].try_into().unwrap()), 2);
    assert_eq!(u64::from_le_bytes(blob[24..32].try_into().unwrap()), 0);
    assert!(blob.windows("enabled".len()).any(|w| w == b"enabled"));
    assert!(blob.windows("channel".len()).any(|w| w == b"channel"));
    assert!(blob.windows(4).any(|w| w == 42u32.to_le_bytes()));
}

#[test]
fn config_compiler_can_read_overrides_from_product_definition() {
    let manifest = manifest_with_schema(
        "com.example.configured",
        vec![field("limit", 2, true, Some(value_u32(7)), 0)],
    );
    let product = product(
        "qemu_dev",
        "//device/virtual/qemu/base/aarch64",
        ":pcfg",
        vec![],
        vec![override_values(
            "com.example.configured",
            vec![assignment("limit", value_u32(11))],
        )],
    );

    let blob = compile_config_blob_from_product(&manifest, &product, "com.example.configured")
        .expect("product override should compile");

    assert!(blob.windows(4).any(|w| w == 11u32.to_le_bytes()));
}

#[test]
fn config_compiler_generates_rust_bindings() {
    let manifest = manifest_with_schema(
        "com.example.configured",
        vec![
            field("enabled", 1, true, Some(value_bool(false)), 0),
            field("optional_bytes", 5, false, None, 4),
        ],
    );

    let generated =
        bexos_assembly::generate_config_rust(&manifest).expect("config rust should generate");

    assert!(generated.contains("pub struct ComponentConfig"));
    assert!(generated.contains("pub enabled: bool"));
    assert!(generated.contains("pub optional_bytes: Option<Vec<u8>>"));
    assert!(generated.contains("SCHEMA_FINGERPRINT"));
}

#[test]
fn config_compiler_generates_required_field_without_default() {
    let manifest = manifest_with_schema(
        "com.example.configured",
        vec![field("channel", 3, true, None, 0)],
    );

    let generated =
        bexos_assembly::generate_config_rust(&manifest).expect("config rust should generate");

    assert!(generated.contains("pub channel: u64"));
    assert!(generated.contains("channel\")?"));
}

#[test]
fn config_compiler_rejects_unknown_bad_type_missing_and_bounds() {
    let manifest = manifest_with_schema(
        "com.example.configured",
        vec![field("channel", 4, true, None, 4)],
    );

    let missing = compile_config_blob(CompileConfigInput {
        manifest: &manifest,
        override_bytes: None,
    })
    .expect_err("required field should be enforced");
    assert!(missing.contains("missing required"));

    let unknown = override_values(
        "com.example.configured",
        vec![assignment("other", value_string("x"))],
    );
    assert!(
        compile_config_blob(CompileConfigInput {
            manifest: &manifest,
            override_bytes: Some(&unknown),
        })
        .expect_err("unknown field should fail")
        .contains("unknown config field")
    );

    let bad_type = override_values(
        "com.example.configured",
        vec![assignment("channel", value_u64(1))],
    );
    assert!(
        compile_config_blob(CompileConfigInput {
            manifest: &manifest,
            override_bytes: Some(&bad_type),
        })
        .expect_err("bad type should fail")
        .contains("type mismatch")
    );

    let oversized = override_values(
        "com.example.configured",
        vec![assignment("channel", value_string("toolong"))],
    );
    assert!(
        compile_config_blob(CompileConfigInput {
            manifest: &manifest,
            override_bytes: Some(&oversized),
        })
        .expect_err("bounds should fail")
        .contains("exceeds max_size")
    );
}

fn product(
    name: &str,
    board: &str,
    platform_config: &str,
    bundles: Vec<&str>,
    overrides: Vec<Vec<u8>>,
) -> Vec<u8> {
    let mut fields = vec![
        string_field(1, name),
        string_field(2, board),
        string_field(3, platform_config),
    ];
    fields.extend(bundles.into_iter().map(|bundle| string_field(4, bundle)));
    fields.extend(overrides.into_iter().map(|value| bytes_field(5, &value)));
    concat(fields)
}

fn bundle(name: &str, packages: Vec<Vec<u8>>) -> Vec<u8> {
    let mut fields = vec![string_field(1, name)];
    fields.extend(packages.into_iter().map(|package| bytes_field(2, &package)));
    concat(fields)
}

fn package(label: &str, package_id: &str, placement: u64) -> Vec<u8> {
    concat(vec![
        string_field(1, label),
        string_field(2, package_id),
        varint_field(3, placement),
    ])
}

fn manifest_with_schema(package_id: &str, fields: Vec<Vec<u8>>) -> Vec<u8> {
    concat(vec![
        string_field(1, package_id),
        string_field(2, package_id),
        message_field(
            10,
            fields
                .into_iter()
                .map(|field| bytes_field(1, &field))
                .collect(),
        ),
    ])
}

fn library_manifest(package_id: &str, abi_version: u64) -> Vec<u8> {
    concat(vec![
        varint_field(20, bexos_app_manifest::Architecture::current_guest() as u64),
        string_field(1, package_id),
        string_field(2, package_id),
        varint_field(11, 1),
        bytes_field(
            16,
            &concat(vec![
                string_field(1, "runtime"),
                string_field(2, "/pkg/lib/libruntime.so"),
                string_field(3, "runtime_"),
                varint_field(4, abi_version),
            ]),
        ),
    ])
}

fn wasm_component_library_manifest(package_id: &str, abi_version: u64) -> Vec<u8> {
    concat(vec![
        varint_field(20, bexos_app_manifest::Architecture::current_guest() as u64),
        string_field(1, package_id),
        string_field(2, package_id),
        varint_field(11, 1),
        bytes_field(
            16,
            &concat(vec![
                string_field(1, "bexos:wasm/dioxus@1.0.0"),
                string_field(2, "/pkg/lib/dioxus.wasm"),
                varint_field(4, abi_version),
                varint_field(6, 1),
            ]),
        ),
    ])
}

fn manifest_with_dependency(package_id: &str, dependency: &str, abi_version: u64) -> Vec<u8> {
    concat(vec![
        string_field(1, package_id),
        string_field(2, package_id),
        bytes_field(
            12,
            &concat(vec![
                string_field(1, dependency),
                varint_field(4, abi_version),
            ]),
        ),
    ])
}

fn manifest_exposing_service(
    package_id: &str,
    service: &str,
    visibility: u64,
    capability: &str,
    ordinal: u64,
) -> Vec<u8> {
    concat(vec![
        string_field(1, package_id),
        string_field(2, package_id),
        bytes_field(
            5,
            &concat(vec![
                string_field(1, service),
                varint_field(4, visibility),
                bytes_field(
                    7,
                    &concat(vec![string_field(1, capability), varint_field(3, ordinal)]),
                ),
            ]),
        ),
    ])
}

fn manifest_consuming_service(
    package_id: &str,
    service: &str,
    link_type: u64,
    capability: &str,
    ordinal: u64,
) -> Vec<u8> {
    concat(vec![
        string_field(1, package_id),
        string_field(2, package_id),
        bytes_field(
            6,
            &concat(vec![
                string_field(1, service),
                varint_field(2, link_type),
                bytes_field(
                    4,
                    &concat(vec![
                        string_field(1, capability),
                        bytes_field(
                            2,
                            &concat(vec![varint_field(1, ordinal), varint_field(2, 1)]),
                        ),
                    ]),
                ),
            ]),
        ),
    ])
}

fn field(
    name: &str,
    config_type: u64,
    required: bool,
    default_value: Option<Vec<u8>>,
    max_size: u64,
) -> Vec<u8> {
    let mut fields = vec![
        string_field(1, name),
        varint_field(2, config_type),
        varint_field(3, u64::from(required)),
        varint_field(5, max_size),
    ];
    if let Some(default_value) = default_value {
        fields.push(bytes_field(4, &default_value));
    }
    concat(fields)
}

fn override_values(package_id: &str, assignments: Vec<Vec<u8>>) -> Vec<u8> {
    let mut fields = vec![string_field(1, package_id)];
    fields.extend(
        assignments
            .into_iter()
            .map(|assignment| bytes_field(2, &assignment)),
    );
    concat(fields)
}

fn assignment(name: &str, value: Vec<u8>) -> Vec<u8> {
    concat(vec![string_field(1, name), bytes_field(2, &value)])
}

fn value_bool(value: bool) -> Vec<u8> {
    varint_field(1, u64::from(value))
}

fn value_u32(value: u32) -> Vec<u8> {
    varint_field(2, value.into())
}

fn value_u64(value: u64) -> Vec<u8> {
    varint_field(3, value)
}

fn value_string(value: &str) -> Vec<u8> {
    string_field(4, value)
}

#[test]
fn product_locks_are_separate_from_config_and_validate_scope() {
    let mut editable = field("dark", 1, false, Some(value_bool(false)), 0);
    editable.extend(varint_field(6, 2));
    let manifest = manifest_with_schema("app.test", vec![editable]);
    let overrides = concat(vec![string_field(1, "app.test"), string_field(3, "dark")]);
    let product = product("test", ":board", ":platform", vec![], vec![overrides]);
    let policy = bexos_assembly::compile_config_policy(&manifest, &product, "app.test").unwrap();
    assert!(!policy.is_empty());
    let manifest = manifest_with_schema(
        "app.test",
        vec![field("dark", 1, false, Some(value_bool(false)), 0)],
    );
    assert!(bexos_assembly::compile_config_policy(&manifest, &product, "app.test").is_err());
}
