use bexos_appd::{
    ElfRunnerOptions, Manifest, Process, ProcessRunnerOptions,
    commands::{self, CommandError, ResolvedCommand},
    manifest::BinaryCommand,
};
fn manifest() -> Manifest {
    Manifest {
        package_name: "example.tools".into(),
        processes: vec![Process {
            name: "command".into(),
            runner: "elf".into(),
            runner_options: Some(ProcessRunnerOptions::Elf(ElfRunnerOptions {
                path: "/pkg/bin/tool".into(),
            })),
            ..Default::default()
        }],
        commands: vec![BinaryCommand {
            command_name: "tool".into(),
            process_name: "command".into(),
        }],
        ..Default::default()
    }
}
#[test]
fn declarations_require_safe_names_and_launchable_processes() {
    let good = manifest();
    assert!(good.validate_commands().is_ok());
    for name in [
        "",
        ".",
        "..",
        "../tool",
        "package:tool",
        "bad name",
        "bad\0name",
    ] {
        let mut m = good.clone();
        m.commands[0].command_name = name.into();
        assert!(m.validate_commands().is_err(), "{name:?}");
    }
    let mut m = good.clone();
    m.commands.push(m.commands[0].clone());
    assert!(m.validate_commands().is_err());
    let mut m = good.clone();
    m.commands[0].process_name = "missing".into();
    assert!(m.validate_commands().is_err());
    let mut m = good.clone();
    m.processes[0].service = true;
    assert!(m.validate_commands().is_err());
    for path in [
        "/data/tool",
        "/pkg/../tool",
        "/pkg/./tool",
        "/pkg/bin/",
        "/pkg/a\0b",
    ] {
        let mut m = good.clone();
        m.processes[0].runner_options = Some(ProcessRunnerOptions::Elf(ElfRunnerOptions {
            path: path.into(),
        }));
        assert!(m.validate_commands().is_err(), "{path:?}");
    }
}
#[test]
fn collisions_are_explicit_and_package_qualification_is_exact() {
    let command = |package: &str| ResolvedCommand {
        package: package.into(),
        process: "command".into(),
        name: "tool".into(),
        executable: "/pkg/bin/tool".into(),
    };
    let a = command("a.tools");
    let b = command("b.tools");
    assert_eq!(
        commands::resolve(&[a.clone()], "/system/bin/tool"),
        Ok(a.clone())
    );
    let all = [a.clone(), b.clone()];
    assert_eq!(
        commands::resolve(&all, "tool"),
        Err(CommandError::Ambiguous(vec![
            "a.tools:tool".into(),
            "b.tools:tool".into()
        ]))
    );
    assert_eq!(commands::resolve(&all, "b.tools:tool"), Ok(b));
    for name in ["missing", "a:tool", "/system/bin/../tool", "tool/x"] {
        assert_eq!(commands::resolve(&all, name), Err(CommandError::NotFound));
    }
    assert_eq!(CommandError::NotFound.exit_status(), 127);
    assert_eq!(CommandError::InvalidManifest.exit_status(), 126);
}

#[test]
fn command_controls_checkpoint_identity_waits_and_completed_status() {
    use bexos_appd::{
        OpenerBinding,
        command_state::{CommandState, ControlledProcess},
    };
    let state = CommandState {
        bindings: vec![OpenerBinding {
            channel: 1,
            package: "shell".into(),
            uid: 1001,
            system: false,
        }],
        processes: vec![ControlledProcess {
            channel: 2,
            process: 3,
            package: "tools".into(),
            uid: 1001,
            watch_pending: true,
            completion: Some(137),
            resource_group: 4,
        }],
    };
    let bytes = state.encode();
    assert_eq!(CommandState::decode(&bytes).unwrap(), state);
    for n in 0..bytes.len() {
        assert!(CommandState::decode(&bytes[..n]).is_err());
    }
    let mut invalid = state.clone();
    invalid.processes[0].channel = 1;
    assert!(CommandState::decode(&invalid.encode()).is_err());
    let mut invalid = state.clone();
    invalid.bindings[0].system = true;
    assert!(CommandState::decode(&invalid.encode()).is_err());
    let mut detached = state;
    detached.processes[0].channel = 0;
    detached.processes[0].watch_pending = false;
    assert_eq!(CommandState::decode(&detached.encode()).unwrap(), detached);
}

#[test]
fn process_control_cannot_cross_user_identities() {
    assert!(commands::can_control(0, 42));
    assert!(commands::can_control(42, 42));
    assert!(!commands::can_control(42, 0));
    assert!(!commands::can_control(42, 43));
}

#[test]
fn bazel_manifest_preserves_commands_and_architecture_as_distinct_fields() {
    let manifest = Manifest::decode(include_bytes!(env!("NATIVE_COMMAND_MANIFEST"))).unwrap();
    assert_eq!(
        manifest.architecture,
        bexos_app_manifest::Architecture::current_guest()
    );
    assert_eq!(manifest.commands.len(), 1);
    assert_eq!(manifest.commands[0].command_name, "probe");
    assert_eq!(manifest.commands[0].process_name, "probe");
}
