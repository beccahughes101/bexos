use bexos_package_version::{
    PackageSelector, SemVer, VersionMatchError, parse_package_selector, select_unique_match,
    split_versioned_package, versioned_package_key,
};

#[test]
fn parses_versioned_package_without_confusing_app_namespace() {
    assert_eq!(split_versioned_package("com.example:demo"), None);
    assert_eq!(
        split_versioned_package("com.bexos.lib.react_native:v0.74"),
        Some(("com.bexos.lib.react_native", "v0.74"))
    );
}

#[test]
fn package_selector_accepts_package_plus_requirement() {
    let selector = parse_package_selector("com.bexos.lib.react_native", Some("v0.74")).unwrap();
    assert_eq!(selector.package, "com.bexos.lib.react_native");
    assert_eq!(selector.version_requirement.as_deref(), Some("v0.74"));
}

#[test]
fn versioned_package_key_omits_legacy_version() {
    assert_eq!(
        versioned_package_key("com.example:demo", &SemVer::default()),
        "com.example:demo"
    );
    assert_eq!(
        versioned_package_key(
            "com.bexos.lib.react_native",
            &SemVer {
                major: 0,
                minor: 74,
                patch: 3,
                build: 12,
                prerelease: String::new(),
            }
        ),
        "com.bexos.lib.react_native:0.74.3-b12"
    );
}

#[test]
fn select_unique_partial_match_rejects_ambiguity() {
    let versions = vec![
        ("com.bexos.lib.react_native", semver(0, 74, 1)),
        ("com.bexos.lib.react_native", semver(0, 74, 2)),
    ];
    let selector = PackageSelector {
        package: "com.bexos.lib.react_native".into(),
        version_requirement: Some("v0.74".into()),
    };

    assert_eq!(
        select_unique_match(&versions, &selector, |item| (item.0, &item.1)),
        Err(VersionMatchError::Ambiguous)
    );
}

#[test]
fn select_unique_partial_match_finds_single_candidate() {
    let versions = vec![
        ("com.bexos.lib.react_native", semver(0, 73, 9)),
        ("com.bexos.lib.react_native", semver(0, 74, 2)),
    ];
    let selector = PackageSelector {
        package: "com.bexos.lib.react_native".into(),
        version_requirement: Some("v0.74".into()),
    };

    assert_eq!(
        select_unique_match(&versions, &selector, |item| (item.0, &item.1)).unwrap(),
        &versions[1]
    );
}

fn semver(major: u32, minor: u32, patch: u32) -> SemVer {
    SemVer {
        major,
        minor,
        patch,
        build: 0,
        prerelease: String::new(),
    }
}
