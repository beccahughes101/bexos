//! Ordered lookup candidates for capability-backed command directories.
use std::path::{Path, PathBuf};
pub enum Candidate {
    Installed(String),
    File(PathBuf),
}
pub fn candidates(name: &str, path: &[PathBuf], cwd: &Path) -> Vec<Candidate> {
    fn candidate(path: PathBuf) -> Candidate {
        if let Some(name) = path.to_str().and_then(|p| p.strip_prefix("/pkg/bin/")) {
            if !name.contains('/') {
                return Candidate::Installed(format!("bexos.app.brush_shell:{name}"));
            }
        }
        if path.to_str().is_some_and(|p| p.starts_with("/system/bin/")) {
            return Candidate::Installed(path.to_string_lossy().into_owned());
        }
        Candidate::File(path)
    }
    if name.contains(':') && !name.contains('/') {
        return vec![Candidate::Installed(name.into())];
    }
    if name.contains('/') {
        return vec![candidate(cwd.join(name))];
    }
    path.iter()
        .map(|directory| candidate(cwd.join(directory).join(name)))
        .collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn path_order_and_package_qualification_are_explicit() {
        let found = candidates(
            "cat",
            &["/system/bin".into(), "/pkg/bin".into(), ".".into()],
            Path::new("/data"),
        );
        assert!(matches!(&found[0],Candidate::Installed(name) if name=="/system/bin/cat"));
        assert!(
            matches!(&found[1],Candidate::Installed(name) if name=="bexos.app.brush_shell:cat")
        );
        assert!(matches!(&found[2],Candidate::File(path) if path==Path::new("/data/./cat")));
        assert!(
            matches!(&candidates("other:cat",&[],Path::new("/data"))[0],Candidate::Installed(name) if name=="other:cat")
        );
        assert!(candidates("cat", &[], Path::new("/data")).is_empty());
    }
}
