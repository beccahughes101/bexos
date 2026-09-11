"""Build Rust WASI 0.2 components using the registered Bazel Rust toolchain."""
load("@rules_rust//rust:defs.bzl", "rust_binary")

def _wasi_transition_impl(settings, attr):
    return {"//command_line_option:platforms": "//build/platforms:wasip2_guest", "//command_line_option:compilation_mode": "opt"}

_wasi_transition = transition(
    implementation = _wasi_transition_impl,
    inputs = [],
    outputs = ["//command_line_option:platforms", "//command_line_option:compilation_mode"],
)

def _component_impl(ctx):
    return [DefaultInfo(files = ctx.attr.binary[0][DefaultInfo].files)]

_component = rule(
    implementation = _component_impl,
    attrs = {
        "binary": attr.label(cfg = _wasi_transition, mandatory = True),
        "_allowlist_function_transition": attr.label(default = "@bazel_tools//tools/allowlists/function_transition_allowlist"),
    },
)

def wasi_component(name, srcs, deps, crate_name, **kwargs):
    rust_binary(name = name + "_binary", srcs = srcs, deps = deps, crate_name = crate_name, edition = "2024", **kwargs)
    _component(name = name, binary = ":" + name + "_binary")
