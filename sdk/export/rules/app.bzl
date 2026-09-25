load("@rules_rust//rust:defs.bzl", "rust_binary")

_ASSEMBLY = Label("//tools:bexos_assembly")
_ARCHIVE = Label("//tools:bex_archive")
_MANIFEST_STAMP = Label("//tools:manifest_stamp")
_PROTO = Label("//idl:bexos/app/manifest.proto")
_VERSION_PROTO = Label("//idl:bexos/version/version.proto")
_RUNTIME = Label("//rust:bexos_app")
_COMPONENT_RUNTIME = Label("//rust:bexos_component")
_STD_RUNTIME = Label("//rust:bexos_libc")
_USERSPACE_RUNTIME = Label("//rust:bexos_driver_startup")
_STD_STUBS = Label("//lib/bexos_libc:empty_libs")
_AARCH64_PLATFORM = Label("//platforms:native_aarch64")
_X86_64_PLATFORM = Label("//platforms:native_x86_64")
_WASIP2_PLATFORM = Label("//platforms:wasip2")
_AARCH64_LINKER = Label("//rules:linker/aarch64.ld")
_X86_64_LINKER = Label("//rules:linker/x86_64.ld")
_PROTOC = Label("@protobuf//:protoc")
_WELL_KNOWN_PROTOS = Label("@protobuf//:well_known_type_protos")

def _std_linker_stubs_impl(ctx):
    out = ctx.actions.declare_directory(ctx.label.name)
    args = ctx.actions.args()
    args.add(out.path)
    args.add_all(ctx.files.srcs)
    ctx.actions.run_shell(
        inputs = ctx.files.srcs,
        outputs = [out],
        arguments = [args],
        command = "out=$1; shift; mkdir -p \"$out\"; for src in \"$@\"; do cp \"$src\" \"$out/${src##*/}\"; done",
        mnemonic = "BexosStdLinkerStubs",
    )
    return [DefaultInfo(files = depset([out]))]

_std_linker_stubs = rule(
    implementation = _std_linker_stubs_impl,
    attrs = {"srcs": attr.label_list(allow_files = True)},
)

def bexos_app_manifest(name, src, architecture):
    if architecture not in ["AARCH64", "X86_64", "MULTI"]:
        fail("architecture must be AARCH64, X86_64, or MULTI")
    native.genrule(
        name = name,
        srcs = [src, _PROTO, _VERSION_PROTO, _WELL_KNOWN_PROTOS],
        outs = [name + ".bexmanifest"],
        cmd = "SDK_ROOT=$(location " + str(_PROTO) + "); SDK_ROOT=$${SDK_ROOT%/idl/bexos/app/manifest.proto}; " +
              "PROTOBUF_SRC=; for proto in $(locations " + str(_WELL_KNOWN_PROTOS) + "); do case $$proto in */google/protobuf/any.proto) PROTOBUF_SRC=$${proto%/google/protobuf/any.proto};; esac; done; " +
              "$(location " + str(_PROTOC) + ") --proto_path=$$SDK_ROOT --proto_path=$$PROTOBUF_SRC " +
              "--encode=bexos.app.Manifest $(location " + str(_PROTO) + ") < $(location %s) " % src +
              "| $(location %s) %s > $@" % (_MANIFEST_STAMP, architecture),
        tools = [_PROTOC, _MANIFEST_STAMP],
    )

def bexos_app_archive(name, manifest, signing_key, entries, compression = "zstd", component_type = "application"):
    if component_type not in ["application", "service", "driver"]:
        fail("component_type must be application, service, or driver")
    config = name + "_default_config"
    native.genrule(
        name = config,
        srcs = [manifest],
        outs = [config + ".bexconfig"],
        cmd = "$(location %s) config --manifest-bin $(location %s) --out $@" % (_ASSEMBLY, manifest),
        tools = [_ASSEMBLY],
    )
    all_entries = dict(entries)
    all_entries["config/component.bexconfig"] = ":" + config
    command = "$(location %s) create --sdk-component %s --out $@ --manifest $(location %s) --key $(location %s) --compression %s" % (
        _ARCHIVE,
        component_type,
        manifest,
        signing_key,
        compression,
    )
    for path, src in sorted(all_entries.items()):
        command += " --entry %s=$(location %s)" % (path, src)
    native.genrule(
        name = name,
        srcs = [manifest, signing_key] + all_entries.values(),
        outs = [name + ".bex"],
        cmd = command,
        tools = [_ARCHIVE],
    )

def bexos_wasm_app(name, srcs, manifest, signing_key, crate_name = None, deps = [], data = {}, compression = "zstd"):
    binary = name + "_wasm"
    rust_binary(
        name = binary,
        srcs = srcs,
        crate_root = srcs[0],
        crate_name = crate_name or name,
        edition = "2024",
        platform = _WASIP2_PLATFORM,
        deps = deps,
    )
    compiled_manifest = name + "_manifest"
    bexos_app_manifest(name = compiled_manifest, src = manifest, architecture = "MULTI")
    entries = dict(data)
    entries["bin/%s.wasm" % name] = ":" + binary
    bexos_app_archive(
        name = name,
        manifest = ":" + compiled_manifest,
        signing_key = signing_key,
        entries = entries,
        compression = compression,
    )

def _bexos_native_component(name, srcs, manifest, signing_key, architecture, component_type, crate_name = None, binary_name = None, deps = [], fidl_deps = [], link_deps = [], data = {}, compression = "zstd", std = False):
    if architecture == "aarch64":
        platform = _AARCH64_PLATFORM
        linker = _AARCH64_LINKER
        manifest_architecture = "AARCH64"
    elif architecture == "x86_64":
        platform = _X86_64_PLATFORM
        linker = _X86_64_LINKER
        manifest_architecture = "X86_64"
    else:
        fail("native architecture must be aarch64 or x86_64")
    binary = name + "_elf"
    runtime_deps = [_COMPONENT_RUNTIME, _USERSPACE_RUNTIME]
    rustc_flags = [
        "--cfg=bexos_guest",
        "-C", "panic=abort",
        "-C", "relocation-model=static",
        "-C", "opt-level=2",
        "-C", "debuginfo=0",
        "-C", "linker=rust-lld",
        "-C", "default-linker-libraries=no",
        "-C", "link-arg=-nostdlib",
        "-C", "link-arg=--strip-debug",
        "-C", "link-arg=-z",
        "-C", "link-arg=max-page-size=4096",
    ]
    compile_data = []
    if std:
        runtime_deps.append(_STD_RUNTIME)
        stub_target = name + "_std_linker_stubs"
        _std_linker_stubs(name = stub_target, srcs = [_STD_STUBS])
        compile_data.append(":" + stub_target)
        rustc_flags.extend(["-L", "native=$(execpath :%s)" % stub_target])
    rust_binary(
        name = binary,
        srcs = srcs,
        crate_root = srcs[0],
        crate_name = crate_name or name,
        edition = "2024",
        platform = platform,
        linker_script = linker,
        rustc_flags = rustc_flags,
        compile_data = compile_data,
        deps = runtime_deps + deps + fidl_deps,
        link_deps = link_deps,
    )
    compiled_manifest = name + "_manifest"
    bexos_app_manifest(name = compiled_manifest, src = manifest, architecture = manifest_architecture)
    entries = dict(data)
    entries["bin/%s" % (binary_name or name)] = ":" + binary
    bexos_app_archive(
        name = name,
        manifest = ":" + compiled_manifest,
        signing_key = signing_key,
        entries = entries,
        compression = compression,
        component_type = component_type,
    )

def bexos_native_app(name, srcs, manifest, signing_key, architecture, crate_name = None, binary_name = None, deps = [], data = {}, compression = "zstd"):
    _bexos_native_component(
        name = name,
        srcs = srcs,
        manifest = manifest,
        signing_key = signing_key,
        architecture = architecture,
        component_type = "application",
        crate_name = crate_name,
        binary_name = binary_name,
        deps = [_RUNTIME] + deps,
        data = data,
        compression = compression,
    )

def bexos_service(name, srcs, manifest, signing_key, architecture, crate_name = None, binary_name = None, deps = [], fidl_deps = [], link_deps = [], data = {}, compression = "zstd", std = False):
    _bexos_native_component(
        name = name,
        srcs = srcs,
        manifest = manifest,
        signing_key = signing_key,
        architecture = architecture,
        component_type = "service",
        crate_name = crate_name,
        binary_name = binary_name,
        deps = deps,
        fidl_deps = fidl_deps,
        link_deps = link_deps,
        data = data,
        compression = compression,
        std = std,
    )

def bexos_driver(name, srcs, manifest, signing_key, architecture, crate_name = None, binary_name = None, deps = [], fidl_deps = [], link_deps = [], data = {}, compression = "zstd", std = False):
    _bexos_native_component(
        name = name,
        srcs = srcs,
        manifest = manifest,
        signing_key = signing_key,
        architecture = architecture,
        component_type = "driver",
        crate_name = crate_name,
        binary_name = binary_name,
        deps = deps,
        fidl_deps = fidl_deps,
        link_deps = link_deps,
        data = data,
        compression = compression,
        std = std,
    )
