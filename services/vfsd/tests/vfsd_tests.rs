use bexos_package_version::{MultiVersionPolicy, SemVer};
use bexos_vfsd::{
    app_data_directory_path, app_data_directory_path_for_policy, is_staged_generation_archive,
    package_archive_path, shared_vault_directory_path, user_filesystem_path,
};
use fs_fidl::FsStatus;

#[test]
fn only_exact_generation_archive_names_use_joint_registry_durability() {
    assert!(is_staged_generation_archive(
        "bexos.app.sysui.generation_101.0123456789abcdef0123456789abcdef"
    ));
    for package in [
        "bexos.app.generation_tool",
        "bexos.app.generation_101",
        "bexos.app.generation_101.short",
        "bexos.app.generation_.0123456789abcdef0123456789abcdef",
    ] {
        assert!(!is_staged_generation_archive(package), "{package}");
    }
}

#[test]
fn package_archive_path_appends_bex_extension() {
    assert_eq!(
        package_archive_path("bexos.platform.storage_verify").unwrap(),
        "pkg/bexos.platform.storage_verify.bex"
    );
    assert_eq!(
        package_archive_path("com.bexos.browser").unwrap(),
        "pkg/com.bexos.browser.bex"
    );
    assert_eq!(
        package_archive_path("com.example:demo").unwrap(),
        "pkg/com.example:demo.bex"
    );
    assert_eq!(
        package_archive_path("com.bexos.lib.react_native:v0.74").unwrap(),
        "pkg/com.bexos.lib.react_native/v0.74/pkg.bex"
    );
}

#[test]
fn package_archive_path_rejects_ambient_paths() {
    assert_eq!(package_archive_path("").unwrap_err(), FsStatus::InvalidArgs);
    assert_eq!(
        package_archive_path("../boot").unwrap_err(),
        FsStatus::InvalidArgs
    );
    assert_eq!(
        package_archive_path("pkg/storage_verify").unwrap_err(),
        FsStatus::InvalidArgs
    );
}

#[test]
fn app_data_directory_path_scopes_by_uid_and_package() {
    assert_eq!(
        app_data_directory_path(0, "bexos.platform.storage_verify").unwrap(),
        "data/users/0/apps/bexos.platform.storage_verify/data"
    );
    assert_eq!(
        app_data_directory_path(42, "com.example:demo").unwrap(),
        "data/users/42/apps/com.example:demo/data"
    );
}

#[test]
fn app_data_directory_path_follows_multi_version_policy() {
    let version = SemVer {
        major: 1,
        minor: 2,
        patch: 3,
        build: 4,
        prerelease: String::new(),
    };

    assert_eq!(
        app_data_directory_path_for_policy(
            42,
            "com.example:demo",
            &version,
            MultiVersionPolicy::ParallelExecution
        )
        .unwrap(),
        "data/users/42/apps/com.example:demo/1.2.3-b4/data"
    );
    assert_eq!(
        app_data_directory_path_for_policy(
            42,
            "com.example:demo",
            &version,
            MultiVersionPolicy::SharedStorageMulti
        )
        .unwrap(),
        "data/users/42/apps/com.example:demo/shared/data"
    );
}

#[test]
fn app_data_directory_path_rejects_ambient_paths() {
    assert_eq!(
        app_data_directory_path(0, "../boot").unwrap_err(),
        FsStatus::InvalidArgs
    );
    assert_eq!(
        app_data_directory_path(0, "pkg/storage_verify").unwrap_err(),
        FsStatus::InvalidArgs
    );
}

#[test]
fn user_filesystem_path_scopes_by_uid() {
    assert_eq!(user_filesystem_path(1000).unwrap(), "data/users/1000");
}

#[test]
fn shared_vault_directory_path_scopes_by_runtime_context() {
    assert_eq!(
        shared_vault_directory_path(1000, "", "settings_bridge", false).unwrap(),
        "data/users/1000/shared/local/settings_bridge/data"
    );
    assert_eq!(
        shared_vault_directory_path(1000, "google.com", "google_shared_vault", false).unwrap(),
        "data/users/1000/shared/domain/google.com/google_shared_vault/data"
    );
    assert_eq!(
        shared_vault_directory_path(0, "", "system_core", true).unwrap(),
        "data/system/shared/local/system_core/data"
    );
}

#[test]
fn shared_vault_directory_path_rejects_ambient_names() {
    assert_eq!(
        shared_vault_directory_path(1000, "", "../escape", false).unwrap_err(),
        FsStatus::InvalidArgs
    );
    assert_eq!(
        shared_vault_directory_path(0, "google.com", "system_core", true).unwrap_err(),
        FsStatus::InvalidArgs
    );
    assert_eq!(
        shared_vault_directory_path(1000, "Google.com", "vault", false).unwrap_err(),
        FsStatus::InvalidArgs
    );
}
