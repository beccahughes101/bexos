use bexos_app_manifest::{Architecture, ManifestArchitecture};
use std::io::Write;
use std::process::{Command, Stdio};
fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(std::env::var(name).unwrap()).unwrap()
}
#[test]
fn bazel_compilation_stamps_native_and_preserves_explicit_portable_content() {
    for (name, arch, native) in [
        ("AUTO", Architecture::current_guest(), true),
        ("ARM", Architecture::Aarch64, true),
        ("X86", Architecture::X86_64, true),
        ("PORTABLE", Architecture::Multi, false),
    ] {
        let policy = ManifestArchitecture::decode(&fixture(name)).unwrap();
        assert_eq!(policy.declared, Some(arch));
        assert_eq!(policy.native, native);
        assert_eq!(policy.validate(Some(arch)), Ok(arch));
    }
}
#[test]
fn compiler_rejects_conflicting_declarations_without_rewriting_them() {
    for (source, selected) in [("ARM", "X86_64"), ("PORTABLE", "AARCH64")] {
        let mut child = Command::new(std::env::var("STAMP").unwrap())
            .arg(selected)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(&fixture(source))
            .unwrap();
        let result = child.wait_with_output().unwrap();
        assert!(!result.status.success());
        assert!(result.stdout.is_empty());
        assert!(String::from_utf8_lossy(&result.stderr).contains("ConflictingArchitecture"));
    }
}
