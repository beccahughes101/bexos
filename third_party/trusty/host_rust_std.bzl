"""Expose the native Rust standard library without host-specific target labels."""

def _stdlib_files_impl(_target, ctx):
    # Preserve the original repository paths beside rustc. The toolchain's
    # provider can instead contain files relocated into a generated sysroot.
    return [OutputGroupInfo(host_std = ctx.rule.attr.rust_std[DefaultInfo].files)]

_stdlib_files = aspect(implementation = _stdlib_files_impl)

def _host_rust_std_impl(ctx):
    files = ctx.attr.toolchain[OutputGroupInfo].host_std
    return [DefaultInfo(files = files, runfiles = ctx.runfiles(transitive_files = files))]

host_rust_std = rule(
    implementation = _host_rust_std_impl,
    attrs = {
        "toolchain": attr.label(
            default = "@trusty_rust_1_80_1//:rust_toolchain",
            cfg = "exec",
            aspects = [_stdlib_files],
            providers = [platform_common.ToolchainInfo],
        ),
    },
)
