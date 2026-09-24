use crate::{ProcessRunnerOptions, manifest::Process};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MigrationAdapterKind {
    Elf,
    Wasm,
    Nix,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MigrationLaunchDescriptor<'a> {
    pub kind: MigrationAdapterKind,
    pub executable_path: &'a str,
}

pub fn prepare(process: &Process) -> Result<MigrationLaunchDescriptor<'_>, &'static str> {
    match &process.runner_options {
        Some(ProcessRunnerOptions::Elf(options)) => Ok(MigrationLaunchDescriptor {
            kind: MigrationAdapterKind::Elf,
            executable_path: &options.path,
        }),
        Some(ProcessRunnerOptions::Wasm(options)) => Ok(MigrationLaunchDescriptor {
            kind: MigrationAdapterKind::Wasm,
            executable_path: &options.path,
        }),
        Some(ProcessRunnerOptions::Nix(options)) => Ok(MigrationLaunchDescriptor {
            kind: MigrationAdapterKind::Nix,
            executable_path: &options.path,
        }),
        Some(_) => Err("migration runner adapter missing"),
        None => Err("migration runner options missing"),
    }
}
