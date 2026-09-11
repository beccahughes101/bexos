# RFC 0033: Terminal and shell protocols — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0033](README.md)

## Implementation summary

The implemented terminal is a debugd/bexctl stream bridge to a signed Brush WASI component, with appd command resolution and terminal-control contracts.

## Implemented behavior

- TTY and shell-provider FIDL separate bytes, terminal control, and command resolution. Explicit stdin/stdout/stderr channels carry terminal and external-command I/O.
- Brush supplies shell parsing/builtins; appd resolves declared installed commands and supplies package-scoped namespaces. Debug terminal authentication and shell selection use the existing user/configuration paths.
- Idle shell/service checkpoints preserve supported state and transport identity; appd and debugd have their own migration adapters.

## Gaps and deviations

- Active evaluators, jobs, non-terminal open files, and pending execution continuations defer Brush transplant. Running commands are not proven to survive shell replacement.
- Full signal ownership, Ctrl-Z/fg/bg, shell traps, arbitrary executable PATH files, and some synchronous builtin I/O remain incomplete.
- GUI terminal and SSH frontends are future work. The system-shell regression is not acceptance of all user identities, x86 sessions, or concurrent shell replacements.

## Sources and validation

Implementation and contract evidence: [idl/bexos/tty/pty.fidl](../../../idl/bexos/tty/pty.fidl), [idl/bexos/shell/provider.fidl](../../../idl/bexos/shell/provider.fidl), [apps/brush_shell](../../../apps/brush_shell), [services/appd/src/commands.rs](../../../services/appd/src/commands.rs), [services/debugd/src/shell_runtime.rs](../../../services/debugd/src/shell_runtime.rs), [host/debug_client/src/shell.rs](../../../host/debug_client/src/shell.rs).

Relevant test sources and Bazel targets: [apps/brush_shell/BUILD.bazel](../../../apps/brush_shell/BUILD.bazel), [services/debugd/tests/shell_auth_tests.rs](../../../services/debugd/tests/shell_auth_tests.rs), [tools/bexctl/BUILD.bazel](../../../tools/bexctl/BUILD.bazel).

Detailed guides and previously recorded validation: [brush shell](../../brush-shell.md), [cli](../../cli.md).

This snapshot is based on source, configuration, and test inspection. Runtime,
hardware, performance, and subsystem test results were not newly verified for
this documentation change; linked historical results retain their original scope.
