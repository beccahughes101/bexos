"""Cross compiler binaries always run on the Bazel execution platform."""

load("@llvm//toolchain:selects.bzl", "resource_dir_arg")

def llvm_resource_headers():
    return select({
        "@llvm//platforms/config:%s_%s" % (os, cpu): Label(str(resource_dir_arg(os, cpu)).replace(":compile_resource_dir", ":builtin_resource_dir"))
        for os in ["macos", "linux"]
        for cpu in ["aarch64", "x86_64"]
    })

def _host_tools_impl(ctx):
    inputs = []
    for tool in ctx.attr.tools:
        info = tool[DefaultInfo]
        inputs.append(info.files)
        if info.default_runfiles:
            inputs.append(info.default_runfiles.files)
    return [DefaultInfo(files = depset(transitive = inputs))]

host_tools = rule(
    implementation = _host_tools_impl,
    attrs = {"tools": attr.label_list(cfg = "exec", allow_files = True)},
)
