load("//build/platforms:architecture.bzl", "guest_platform", "guest_select")
load("@rules_rust//rust:defs.bzl", "rust_binary", "rust_shared_library")

def userspace_binary(name, srcs, crate_name, deps, rustc_flags = ["--cfg=bexos_guest",]):
    rust_binary(
        name = name,
        srcs = srcs,
        crate_name = crate_name,
        edition = "2024",
        platform = guest_platform("userspace_freestanding"),
        linker_script = guest_select("//lib/userspace:aarch64.ld", "//lib/userspace:x86_64.ld"),
        rustc_flags = ["--cfg=bexos_guest","-C", "panic=abort", "-C", "relocation-model=static", "-C", "opt-level=2", "-C", "debuginfo=0", "-C", "link-arg=--strip-debug", "-C", "link-arg=-z", "-C", "link-arg=max-page-size=4096"] + rustc_flags,
        deps = deps + ["//lib/userspace", "//idl:kernel_fidl_rust"],
    )

def std_service_binary(name, srcs, crate_name, deps, rustc_flags = ["--cfg=bexos_guest",]):
    rust_binary(
        name = name,
        srcs = srcs,
        crate_name = crate_name,
        edition = "2024",
        platform = guest_platform("userspace"),
        linker_script = guest_select("//lib/userspace:aarch64.ld", "//lib/userspace:x86_64.ld"),
        rustc_flags = ["--cfg=bexos_guest",
            "-C",
            "panic=abort",
            "-C",
            "relocation-model=static",
            "-C",
            "opt-level=2",
            "-C",
            "debuginfo=0",
            "-C",
            "linker=rust-lld",
            "-C",
            "default-linker-libraries=no",
            "-L",
            "native=lib/bexos_libc/empty_libs",
            "-C",
            "link-arg=-nostdlib",
            "-C",
            "link-arg=--strip-debug",
            "-C",
            "link-arg=-z",
            "-C",
            "link-arg=max-page-size=4096",
        ] + rustc_flags,
        compile_data = ["//lib/bexos_libc:empty_libs"],
        deps = deps + ["//lib/bexos_libc", "//lib/userspace", "//idl:kernel_fidl_rust"],
    )

def std_shared_library(name, srcs, crate_name, deps, crate_root = None, crate_features = [], soname = None, rustc_flags = ["--cfg=bexos_guest",]):
    soname_flags = []
    if soname:
        soname_flags = [
            "-C",
            "link-arg=-soname=" + soname,
        ]
    rust_shared_library(
        name = name,
        srcs = srcs,
        crate_root = crate_root,
        crate_name = crate_name,
        crate_features = crate_features,
        edition = "2024",
        platform = guest_platform("userspace"),
        rustc_flags = ["--cfg=bexos_guest",
            "-C",
            "panic=abort",
            "-C",
            "opt-level=2",
            "-C",
            "debuginfo=0",
            "-C",
            "relocation-model=pic",
            "-C",
            "linker=rust-lld",
            "-C",
            "default-linker-libraries=no",
            "-L",
            "native=lib/bexos_libc/empty_libs",
            "-C",
            "link-arg=-nostdlib",
        ] + guest_select(["-C", "link-arg=-z", "-C", "link-arg=force-bti", "-C", "link-arg=-z", "-C", "link-arg=pac-plt"], []) + soname_flags + rustc_flags,
        compile_data = ["//lib/bexos_libc:empty_libs"],
        deps = deps + ["//lib/bexos_libc", "//lib/userspace", "//idl:kernel_fidl_rust"],
    )

def app_manifest(name, src, architecture = "target"):
    """Compile prototxt and stamp the guest CPU before package signing.

    architecture="MULTI" is reserved for portable packages.
    """
    if architecture not in ["target", "MULTI", "AARCH64", "X86_64"]:
        fail("invalid app architecture: " + architecture)
    stamp_command = guest_select(
        " | $(location //tools/app_manifest:stamp) AARCH64 > $@",
        " | $(location //tools/app_manifest:stamp) X86_64 > $@",
    ) if architecture == "target" else " | $(location //tools/app_manifest:stamp) " + architecture + " > $@"
    native.genrule(
        name = name,
        srcs = [src, "//idl:app_manifest_proto_src", "@protobuf//:well_known_type_protos"],
        outs = [name + ".bexmanifest"],
        cmd = "PROTOBUF_SRC=; for proto in $(locations @protobuf//:well_known_type_protos); do case $$proto in */google/protobuf/any.proto) PROTOBUF_SRC=$${proto%/google/protobuf/any.proto};; esac; done; $(location @protobuf//:protoc) --proto_path=. --proto_path=$$PROTOBUF_SRC --encode=bexos.app.Manifest idl/bexos/app/manifest.proto < $(location " + src + ")" + stamp_command,
        tools = ["@protobuf//:protoc", "//tools/app_manifest:stamp"],
    )

def wasm_runner_options(name, src):
    """Encode authored WASM child options using the manifest protobuf schema."""
    native.genrule(
        name = name,
        srcs = [src, "//idl:app_manifest_proto_src", "@protobuf//:well_known_type_protos"],
        outs = [name + ".pb"],
        cmd = "PROTOBUF_SRC=; for proto in $(locations @protobuf//:well_known_type_protos); do case $$proto in */google/protobuf/any.proto) PROTOBUF_SRC=$${proto%/google/protobuf/any.proto};; esac; done; $(location @protobuf//:protoc) --proto_path=. --proto_path=$$PROTOBUF_SRC --encode=bexos.app.WasmRunnerOptions idl/bexos/app/manifest.proto < $(location " + src + ") > $@",
        tools = ["@protobuf//:protoc"],
    )

def nix_runner_options(name, src):
    """Encode authored Starnix child options using the manifest schema."""
    native.genrule(
        name = name,
        srcs = [src, "//idl:app_manifest_proto_src", "@protobuf//:well_known_type_protos"],
        outs = [name + ".pb"],
        cmd = "PROTOBUF_SRC=; for proto in $(locations @protobuf//:well_known_type_protos); do case $$proto in */google/protobuf/any.proto) PROTOBUF_SRC=$${proto%/google/protobuf/any.proto};; esac; done; $(location @protobuf//:protoc) --proto_path=. --proto_path=$$PROTOBUF_SRC --encode=bexos.app.NixRunnerOptions idl/bexos/app/manifest.proto < $(location " + src + ") > $@",
        tools = ["@protobuf//:protoc"],
    )
