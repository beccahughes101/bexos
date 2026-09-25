load("@rules_rust//rust:defs.bzl", "rust_library")

_FIDLC = Label("//tools:fidlc")

def _fidl_rust_impl(ctx):
    out = ctx.actions.declare_file(ctx.attr.out)
    srcs = list(ctx.files.srcs)
    if not srcs:
        fail("bexos_fidl_rust_library requires srcs")
    args = ctx.actions.args()
    args.add_all(srcs)
    for dep in ctx.files.fidl_deps:
        args.add("--dep", dep)
    args.add("--rust-out", out)
    ctx.actions.run(
        executable = ctx.executable._fidlc,
        inputs = srcs + ctx.files.fidl_deps,
        outputs = [out],
        arguments = [args],
        mnemonic = "BexosSdkFidlRust",
    )
    return [DefaultInfo(files = depset([out]))]

_fidl_rust = rule(
    implementation = _fidl_rust_impl,
    attrs = {
        "srcs": attr.label_list(allow_files = [".fidl"]),
        "fidl_deps": attr.label_list(allow_files = [".fidl"]),
        "out": attr.string(mandatory = True),
        "_fidlc": attr.label(default = _FIDLC, executable = True, cfg = "exec"),
    },
)

def bexos_fidl_rust_library(name, srcs, crate_name = None, deps = [], fidl_deps = [], visibility = None):
    generated = name + "_generated"
    _fidl_rust(
        name = generated,
        srcs = srcs,
        fidl_deps = fidl_deps,
        out = name + ".rs",
    )
    rust_library(
        name = name,
        srcs = [":" + generated],
        crate_name = crate_name or name,
        edition = "2024",
        deps = deps,
        visibility = visibility,
    )
