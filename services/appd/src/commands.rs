//! Installed command catalogue derived from the active, verified manifests.
//!
//! Rebuilding from the registry avoids a second persistent index that could
//! diverge during replacement, uninstall, rollback, or appd restoration.
use crate::{Manifest, MemoryAppRegistry, ProcessRunnerOptions};
use alloc::{collections::BTreeSet, string::String, vec::Vec};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedCommand {
    pub package: String,
    pub process: String,
    pub name: String,
    pub executable: String,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CommandError {
    NotFound,
    Ambiguous(Vec<String>),
    InvalidManifest,
}
impl CommandError {
    pub fn exit_status(&self) -> i32 {
        match self {
            Self::NotFound => 127,
            _ => 126,
        }
    }
}

pub fn executable(process: &crate::Process) -> Option<&str> {
    let path = match (&*process.runner, &process.runner_options) {
        ("wasm", Some(ProcessRunnerOptions::Wasm(options))) => &options.path,
        ("elf", Some(ProcessRunnerOptions::Elf(options))) => &options.path,
        _ => return None,
    };
    (path.starts_with("/pkg/")
        && !path.split('/').any(|part| part == ".." || part == ".")
        && !path.contains('\0')
        && !path.ends_with('/'))
    .then_some(path)
}

pub fn catalogue(registry: &MemoryAppRegistry) -> Result<Vec<ResolvedCommand>, CommandError> {
    let mut result = Vec::new();
    let packages: BTreeSet<_> = registry
        .list_packages()
        .iter()
        .map(|r| &r.package_id)
        .collect();
    for package in packages {
        let record = registry
            .record(package)
            .map_err(|_| CommandError::InvalidManifest)?;
        let manifest =
            Manifest::decode(&record.manifest_bytes).map_err(|_| CommandError::InvalidManifest)?;
        for command in &manifest.commands {
            let process = manifest
                .processes
                .iter()
                .find(|p| p.name == command.process_name)
                .ok_or(CommandError::InvalidManifest)?;
            result.push(ResolvedCommand {
                package: package.clone(),
                process: process.name.clone(),
                name: command.command_name.clone(),
                executable: executable(process)
                    .ok_or(CommandError::InvalidManifest)?
                    .into(),
            });
        }
    }
    result.sort_by(|a, b| (&a.name, &a.package).cmp(&(&b.name, &b.package)));
    Ok(result)
}

/// Resolve a name in the installed `/system/bin` directory. A collision is
/// explicit; `package:command` selects one provider deterministically.
/// Local PATH entries are resolved by the caller through its CWD capability.
pub fn resolve(catalogue: &[ResolvedCommand], name: &str) -> Result<ResolvedCommand, CommandError> {
    let name = name.strip_prefix("/system/bin/").unwrap_or(name);
    if name.contains('/') || name.is_empty() {
        return Err(CommandError::NotFound);
    }
    let (package, name) = match name.split_once(':') {
        Some((p, n)) => (Some(p), n),
        None => (None, name),
    };
    let found: Vec<_> = catalogue
        .iter()
        .filter(|c| c.name == name && package.is_none_or(|p| p == c.package))
        .collect();
    match found.as_slice() {
        [] => Err(CommandError::NotFound),
        [command] => Ok((*command).clone()),
        _ => Err(CommandError::Ambiguous(
            found
                .iter()
                .map(|c| alloc::format!("{}:{}", c.package, c.name))
                .collect(),
        )),
    }
}

/// System sessions can administer all appd processes; user sessions retain their identity scope.
pub fn can_control(caller_uid: u64, process_uid: u64) -> bool {
    caller_uid == 0 || caller_uid == process_uid
}
