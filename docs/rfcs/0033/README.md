# RFC 0033: Terminal and shell protocols

- Created: 2026-08-30T10:49:49-05:00
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

Byte streams, terminal control, and shell resolution have separate interfaces. The design retains the implemented Brush and debugd bridge alongside future shell and terminal capabilities.

## Implemented terminal bridge and Brush integration

The current protocol sources are `idl/bexos/tty/pty.fidl` and
`idl/bexos/shell/provider.fidl`; [the CLI reference](../../cli.md) describes
the runnable host bridge. `//apps/brush_shell` builds a signed, storage-installed
WASI 0.2 shell component using Brush revision
`a2620d71bf3d08ca4792655836f9f916c7bb73ff`. Explicit upstream adaptations live in
`third_party/patches/brush-core_bexos.patch`; Bazel generates the bindings and artifacts.

Ownership is explicit: the provider implements `PtySession`. Debugd creates its
control channel and passes the server end to `ShellProvider.CreateSession`. The
provider allocates three socket pairs and transfers the frontend ends once via
`TakeIoStreams`; it retains the stdin-reader and stdout/stderr-writer ends.
`GetStatus` reports exit, and `Close` ends the session. Error replies have absent
stream handles. The shared TTY library owns bounded provider line discipline (16 KiB; a full
canonical chunk is released to keep long lines progressing);
Brush supplies parsing, evaluation, and Bash-mode builtins. `lib/tty` separates
terminal state from native and WASM transports while retaining the original ordinals.

Debugd authenticates user credentials through usersd, binds an identity-scoped
opener through appd, and resolves `bexos.shell.ShellProvider`. System mode is an
explicit developer-debug operation. The BXD1 bridge exchanges bounded chunks,
forwards resize/mode/signal controls, and preserves active frontend state through
heart transplant. The host restores its terminal settings on exit.

The image configuration installs `bexos.app.brush_shell` and selects its `terminal`
process as the fallback ShellProvider. Persisted explicit user preferences take
precedence. The shell launches on demand with identity-scoped `/data`,
`HOME=/data`, and `PATH=/pkg/bin:/system/bin`. GUI terminals and SSH remain future frontends.

Manifest `commands` declarations map names to non-service processes. Appd validates
these against existing ELF/WASM runner options and derives the active catalogue
from installed package records. `Opener.BindCommandLauncher` binds the caller's
identity to command resolution and launch. Launch requests explicitly delegate
CWD and three stdio capabilities; a process-control channel supports status, exit
notification, signals, suspension, and resumption. Appd retains those bindings in
its transplant state. The shared utility component declares `ls`, `cat`, `grep`, `mkdir`, `ps`, and
`kill`. Process listing and signalling are filtered by the launcher identity;
system sessions may administer all appd-managed processes.

**Implementation boundary:** the complete execution design below is not yet
implemented. Brush can checkpoint idle interpreter state, terminal buffers, and
partial source input. A live evaluator, background jobs, or nonstandard open files
currently defer checkpointing; they do not have the required explicit resumable
continuation. Basic builtin pipelines, command substitutions, background execution with `wait`,
and bounded `echo` output run on cooperative tasks and in-process pipes. The
component test exercises these paths. Complete job control, executable scripts
from arbitrary PATH directories, and resumable builtin/I/O progress still need
remaining platform work and acceptance coverage. Installed command lookup follows
PATH order for `/pkg/bin` and `/system/bin`, with explicit package qualification. A deferred checkpoint preserves the working source session. It is not
active-command migration. QEMU acceptance for this change is deferred at the
user's request; image configuration alone is not evidence of a working fresh boot.

The remaining design below describes the longer-term terminal ecosystem. Where
older pseudocode differs, the ownership above and the FIDL sources are authoritative.

The shell and terminal subsystem separates **byte-stream transports**, **terminal line discipline (PTY/TTY control)**, and **shell command resolution**.

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ TERMINAL FRONTENDS (GUI Terminal App, `sshd`, `debugd` Serial Console)     │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ 1. Requests Shell via `Opener`
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ appd / Opener (`GetPreferredInterface("bexos.shell.ShellProvider")`)                │
│ • Resolves user's preferred shell package (e.g. `com.bexos.shell:bash`)     │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │ 2. Spawns Shell with PTY Controller
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ TTY / PTY SESSION LAYER (`bexos.tty.PtySession` + Kernel Sockets)        │
│ • Control Channel: Window resize (`SIGWINCH`), raw/canonical mode, echo     │
│ • Data Channel: Bi-directional lock-free kernel `SOCKET` (stdin/stdout/err) │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ SHELL PROCESS (`bash`, `zsh`, or `bex-sh`)                                  │
│ ├── Shell Builtins: `cd`, `export`, `alias`, `exit` (in-process state)      │
│ ├── Coreutils Bundle: `ls`, `cat`, `ps`, `kill` (static or /deps/coreutils) │
│ └── Dynamic System Commands: Spawns app packages via `appd` or binary path  │
└─────────────────────────────────────────────────────────────────────────────┘

```

## The TTY / PTY FIDL Protocol

Instead of passing character streams through slow IPC serialization, the TTY interface pairs a **kernel `SOCKET` handle** for high-throughput raw byte streams with a **FIDL control protocol** for terminal state, window sizing, and signals.

```fidl
library bexos.tty;

using bexos.kernel;

type WindowSize = struct {
    rows uint16;
    cols uint16;
    pixel_width uint32;
    pixel_height uint32;
};

type TerminalMode = strict bits : uint32 {
    ECHO         = 0x0001; // Echo typed characters back to client
    CANONICAL    = 0x0002; // Line-buffered (Cooked) vs raw byte mode
    ISIG         = 0x0004; // Process Ctrl+C / Ctrl+Z as interrupt events
};

type Signal = strict enum : uint32 {
    INTERRUPT = 1; // Ctrl+C (SIGINT)
    TERMINATE = 2; // SIGTERM
    SUSPEND   = 3; // Ctrl+Z (SIGTSTP)
    KILL      = 4; // Hard terminate
};

/// Interface implemented by the Shell or Process consuming the TTY
protocol PtySession {
    /// Frontend sockets: stdin writer, stdout/stderr readers; transfer once.
    TakeIoStreams() -> (resource struct {
        status bexos.kernel.Status;
        stdin_stream handle:OPTIONAL;
        stdout_stream handle:OPTIONAL;
        stderr_stream handle:OPTIONAL;
    });

    /// Update terminal dimensions on resize (GUI drag, SSH window change)
    SetWindowSize(struct {
        size WindowSize;
    }) -> (struct {
        status bexos.kernel.Status;
    });

    /// Configure line discipline (raw vs cooked)
    SetTerminalMode(struct {
        flags TerminalMode;
    }) -> (struct {
        status bexos.kernel.Status;
    });

    /// Send an out-of-band execution signal to the active foreground process
    SendSignal(struct {
        signal Signal;
    }) -> (struct {
        status bexos.kernel.Status;
    });
    GetStatus() -> (struct {
        status bexos.kernel.Status;
        exited bool;
        exit_code int32;
    });
    Close() -> (struct { status bexos.kernel.Status; });
};

// In library bexos.shell, importing bexos.tty:
/// Interface implemented by Shell Providers (e.g. bash package)
@discoverable
protocol ShellProvider {
    /// Spawn a new interactive shell session bound to the provided PtySession end
    CreateSession(resource struct {
        session server_end:bexos.tty.PtySession;
        environment vector<string:256>:64; // Initial PATH, HOME, TERM, USER
    }) -> (struct {
        status bexos.kernel.Status;
    });
};

```

## Launching the Shell via User Choice

When `debugd`, `sshd`, or the GUI Terminal launches, it does not hardcode `bash`. It asks `appd` for the user's preferred shell:

1. **Resolution:** The terminal caller calls `appd.GetPreferredInterface("bexos.shell.ShellProvider")`.
2. **Preference Lookup:** `appd` checks `/vault/user_<uid>/opener.redb` (falling back to `/system/state/opener.redb`).
3. **Session Creation:** The terminal passes the `PtySession` server endpoint to the provider via `CreateSession()`. The provider allocates three socket pairs; the terminal obtains the frontend endpoints through `TakeIoStreams()`.
4. **User Switchability:** If the user installs `com.bexos.shell:fish` or `com.bexos.shell:zsh`, they can set it as default via `Opener.SetUserDefault("interface:bexos.shell.ShellProvider", "com.bexos.shell:fish")`.

## Command Execution Model in the Shell

Commands executed in the shell fall into three categories:

### A. Shell Built-ins (`cd`, `export`, `set`, `alias`, `exit`)

* **Execution:** Handled **in-process** inside the shell runtime.
* **Why:** In a microkernel, a process's current working directory (CWD) and environment table are local state. Running `cd /data` changes the shell's active VFS directory handle (`vfsd::Directory`), so subsequent path resolutions occur from that handle.

### B. Standard Utilities (`ls`, `cat`, `grep`, `ps`, `mkdir`)

* Packaged either inside the shell package itself or as a mounted library package `/deps/com.bexos.coreutils`.
* When the user types `ls -la`:
* The shell forks or calls `appd.SpawnProcess()` with the shell's cloned namespace, passing the shell's `stdout_stream` socket handle.

### C. Installed App Commands (`monzo`, `waymo`, `vim`)

* When an app like `monzo` or a CLI tool like `vim` is installed, its manifest can declare a CLI binary entry in `manifest.proto`:
```protobuf
message BinaryCommand {
  string command_name = 1; // "monzo"
  string target_entry = 2; // "bin/monzo-cli"
}

```


* When the shell searches `$PATH` (e.g. `/system/bin`, `/deps/...`), `appd` resolves the command name against installed package CLI entries, creates the sandboxed process, and wires its stdin/stdout directly to the active TTY socket.

## Key Advantages of This Design

* **Zero Memory Overhead:** Typing and output bypass message serialization entirely—characters flow across lock-free kernel `SOCKET` rings.
* **Pluggable Shells:** Any package can implement `bexos.shell.ShellProvider` (Bash, Zsh, Nushell, Python REPL, custom debugging shells).
* **Multi-Client Consistency:** GUI Terminal windows, remote SSH sessions (`sshd`), and physical hardware UART serial consoles (`debugd`) talk to the shell using the exact same FIDL TTY protocol.
