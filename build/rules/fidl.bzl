def _bexos_fidl_rust_impl(ctx):
    out = ctx.actions.declare_file(ctx.attr.out)
    srcs = []
    if ctx.file.src:
        srcs.append(ctx.file.src)
    srcs.extend(ctx.files.srcs)
    if not srcs:
        fail("bexos_fidl_rust requires src or srcs")
    ctx.actions.run(
        inputs = srcs + ctx.files.fidl_deps,
        outputs = [out],
        executable = ctx.executable._fidlc,
        arguments = [
            src.path
            for src in srcs
        ] + [
            arg
            for dep in ctx.files.fidl_deps
            for arg in ["--dep", dep.path]
        ] + [
            "--rust-out",
            out.path,
        ],
        mnemonic = "BexosFidlRust",
        progress_message = "Generating Rust FIDL stubs from %s" % ", ".join([src.short_path for src in srcs]),
    )
    return [DefaultInfo(files = depset([out]))]

bexos_fidl_rust = rule(
    implementation = _bexos_fidl_rust_impl,
    attrs = {
        "src": attr.label(
            allow_single_file = [".fidl"],
        ),
        "srcs": attr.label_list(allow_files = [".fidl"]),
        "fidl_deps": attr.label_list(allow_files = [".fidl"]),
        "out": attr.string(mandatory = True),
        "_fidlc": attr.label(
            default = Label("//tools/fidlc:fidlc"),
            executable = True,
            cfg = "exec",
        ),
    },
    doc = "Generates Rust stubs and capability metadata from a BexOS FIDL file.",
)
