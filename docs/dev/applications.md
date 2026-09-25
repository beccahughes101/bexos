# Applications

An out-of-tree application is either a portable WASI component built with
`bexos_wasm_app` or a native BexOS ELF built with `bexos_native_app`. Both rules
compile a prototxt manifest, generate the signed default component
configuration, and emit a signed `.bex` archive.

## Portable WASM application

```starlark
load("@bexos_sdk//rules:defs.bzl", "bexos_wasm_app")

bexos_wasm_app(
    name = "diagnostics",
    srcs = ["src/diagnostics.rs"],
    deps = ["@bexos_sdk//rust:bexos_wasm_guest"],
    manifest = "diagnostics.prototxt",
    signing_key = "signing.key",
    data = {
        "data/defaults.json": "defaults.json",
    },
)
```

The rule builds for `wasm32-wasip2`, stores the module at
`bin/diagnostics.wasm`, and stamps the manifest architecture as `MULTI`. The
manifest path must match the rule output:

```textproto
package_name: "com.acme.diagnostics"
name: "ACME diagnostics"
min_bexos_abi_version: 1
processes {
  name: "diagnostics"
  runner: "wasm"
  service: false
  runner_options {
    [type.googleapis.com/bexos.app.WasmRunnerOptions] {
      path: "/pkg/bin/diagnostics.wasm"
      limits { max_memory_pages: 1024 max_handles: 64 }
    }
  }
}
```

All WASM limits are finite. Omitted values use platform defaults and zero does
not mean unlimited. Arguments, environment entries, and versioned component
imports are also declared in `WasmRunnerOptions`.

If the WASM process is a long-running service, set `service: true`, give it an
appropriate wave when it is eagerly started, select `HEART_TRANSPLANT`, and
implement the lifecycle interface described in [Services](services.md).

## Native application

```starlark
load("@bexos_sdk//rules:defs.bzl", "bexos_native_app")

bexos_native_app(
    name = "diagnostics_aarch64",
    srcs = ["src/native.rs"],
    architecture = "aarch64",
    binary_name = "diagnostics",
    manifest = "diagnostics.prototxt",
    signing_key = "signing.key",
)

bexos_native_app(
    name = "diagnostics_x86_64",
    srcs = ["src/native.rs"],
    architecture = "x86_64",
    binary_name = "diagnostics",
    manifest = "diagnostics.prototxt",
    signing_key = "signing.key",
)
```

The same source manifest is stamped `AARCH64` or `X86_64` by the rule. The
signed archives are not interchangeable. The ELF process uses the path
`/pkg/bin/diagnostics`:

```textproto
processes {
  name: "diagnostics"
  runner: "elf"
  service: false
  runner_options {
    [type.googleapis.com/bexos.app.ELFRunnerOptions] {
      path: "/pkg/bin/diagnostics"
    }
  }
}
```

A freestanding native application uses the `bexos_app` runtime supplied by the
rule:

```rust
#![no_std]
#![no_main]

fn main(startup_channel: u64) -> ! {
    bexos_app::log("diagnostics: started\n");
    let _ = startup_channel;
    loop {
        bexos_app::yield_now();
    }
}

bexos_app::entry!(main);
```

`bexos_native_app` intentionally exposes the small application runtime. Native
services and drivers use `bexos_component` because they must decode structured
startup and participate in migration.

## Assets and configuration

The `data` dictionary maps archive-relative paths to Bazel labels. Use
normalized paths such as `data/theme.json`; absolute paths and `..` traversal
are rejected during product import. Every package rule also generates
`config/component.bexconfig` from the manifest's `config_schema` and signs it
into the archive.

Runtime package files are visible below `/pkg`. Disk-launched components also
receive `/data`, `/tmp`, declared dependency mounts, and other namespace entries
selected by appd policy. Do not assume a Linux root filesystem.

## Commands and launch behavior

An installed application is launched explicitly unless a process is marked as
a service. A manifest can map a shell command to a declared process:

```textproto
commands { command_name: "diagnostics" process_name: "diagnostics" }
```

The command selects the process; the signed runner options still control the
executable path and runtime. Service waves are for dependency-ordered startup,
not command applications.

## Application checklist

- Use `min_bexos_abi_version: 1` and a stable package ID.
- Keep executable paths under `/pkg/bin/` and aligned with `binary_name`.
- Build and release native archives for both architectures you support.
- Keep a WASM archive free of native ELF/shared-library payloads.
- Declare only the permissions, consumed services, resources, and assets the
  application actually needs.
- If any process has `service: true`, implement heart transplant for it.
