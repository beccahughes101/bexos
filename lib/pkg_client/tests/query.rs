use bexos_pkg_client::{ArtifactKind, ArtifactQuery, valid_host, valid_repository};
#[test]
fn rejects_ambiguous_registry_paths() {
    for host in [
        "",
        "REGISTRY.test",
        "user@registry.test",
        "registry.test:0",
        "registry.test:65536",
        "registry.test:08443",
        "registry.test/",
        "-bad.test",
    ] {
        assert!(!valid_host(host));
    }
    assert!(valid_host("registry.test:8443"));
    assert_eq!(
        bexos_pkg_client::split_registry_authority("registry.test:8443"),
        Some(("registry.test", 8443))
    );
    for repo in [
        "",
        "../secret",
        "apps//demo",
        "/apps",
        "apps?q=1",
        "apps/..x",
        "apps/-demo",
        "apps/demo_",
        "apps/a...b",
    ] {
        assert!(!valid_repository(repo));
    }
    let mut query = ArtifactQuery {
        registry_host: "registry.test".into(),
        repository: "apps/demo".into(),
        tag: "v1.0".into(),
        expected_digest: None,
        kind: ArtifactKind::Application,
    };
    assert!(query.validate().is_ok());
    query.tag = "../v2".into();
    assert!(query.validate().is_err());
}

#[test]
fn accepts_oci_repository_separators() {
    for repository in ["apps/demo", "a.b/c_d", "a__b/c---d", "x/y0"] {
        assert!(valid_repository(repository));
    }
}
