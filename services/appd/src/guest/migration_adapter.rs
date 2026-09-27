use crate::{ComponentRunnerProviderKind, ProcessRunnerOptions, manifest::Process};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MigrationLaunchDescriptor<'a> {
    pub executable_path: Option<&'a str>,
    pub restricted_kick: bool,
}

pub fn prepare(
    process: &Process,
    provider_kind: ComponentRunnerProviderKind,
) -> Result<MigrationLaunchDescriptor<'_>, &'static str> {
    match provider_kind {
        ComponentRunnerProviderKind::DirectElf => match &process.runner_options {
            Some(ProcessRunnerOptions::Elf(options)) => Ok(MigrationLaunchDescriptor {
                executable_path: Some(&options.path),
                restricted_kick: false,
            }),
            _ => Err("direct ELF migration options missing"),
        },
        ComponentRunnerProviderKind::ComponentRunner => Ok(MigrationLaunchDescriptor {
            executable_path: None,
            // Restricted-state migration needs the kernel kick to leave guest
            // mode. This is a program ABI property, not runner selection.
            restricted_kick: process
                .runner_program
                .as_ref()
                .is_some_and(|program| program.type_url == bexos_starnix_abi::OPTIONS_TYPE_URL),
        }),
        ComponentRunnerProviderKind::Unspecified => Err("migration runner provider missing"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AnyRunnerOptions, ElfRunnerOptions};

    #[test]
    fn component_runner_migration_is_not_limited_to_built_in_runner_names() {
        let process = Process {
            runner: "custom_runtime".into(),
            runner_options: Some(ProcessRunnerOptions::Unknown(AnyRunnerOptions {
                type_url: "type.example/custom.Options".into(),
                value: vec![1, 2, 3],
            })),
            runner_program: Some(AnyRunnerOptions {
                type_url: "type.example/custom.Options".into(),
                value: vec![1, 2, 3],
            }),
            ..Process::default()
        };
        assert_eq!(
            prepare(&process, ComponentRunnerProviderKind::ComponentRunner),
            Ok(MigrationLaunchDescriptor {
                executable_path: None,
                restricted_kick: false,
            })
        );
    }

    #[test]
    fn direct_elf_and_restricted_component_properties_are_explicit() {
        let direct = Process {
            runner_options: Some(ProcessRunnerOptions::Elf(ElfRunnerOptions {
                path: "/pkg/bin/service".into(),
                stack_size_bytes: 0,
            })),
            ..Process::default()
        };
        assert_eq!(
            prepare(&direct, ComponentRunnerProviderKind::DirectElf),
            Ok(MigrationLaunchDescriptor {
                executable_path: Some("/pkg/bin/service"),
                restricted_kick: false,
            })
        );

        let restricted = Process {
            runner_program: Some(AnyRunnerOptions {
                type_url: bexos_starnix_abi::OPTIONS_TYPE_URL.into(),
                value: vec![1],
            }),
            ..Process::default()
        };
        assert!(
            prepare(&restricted, ComponentRunnerProviderKind::ComponentRunner)
                .unwrap()
                .restricted_kick
        );
    }
}
