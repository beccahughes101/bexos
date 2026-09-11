load("@rules_cc//cc:cc_toolchain_config_lib.bzl", "tool_path")  # buildifier: disable=deprecated-function
load("@rules_cc//cc/common:cc_common.bzl", "cc_common")
load("@rules_cc//cc/toolchains:cc_toolchain_config_info.bzl", "CcToolchainConfigInfo")

def _impl(ctx):
    # LLVM exposes the selected resource directory as a declared artifact.
    # Keep the include root tied to that artifact instead of the host SDK.
    resources = ctx.attr._headers[DefaultInfo].files.to_list()
    if len(resources) != 1:
        fail("expected one LLVM builtin resource directory")
    tool_paths = [
        tool_path(name = "ar", path = "ar.sh"),
        tool_path(name = "cpp", path = ctx.attr.clang_wrapper),
        tool_path(name = "gcc", path = ctx.attr.clang_wrapper),
        tool_path(name = "gcov", path = ctx.attr.clang_wrapper),
        tool_path(name = "ld", path = ctx.attr.clang_wrapper),
        tool_path(name = "nm", path = "nm.sh"),
        tool_path(name = "objdump", path = "nm.sh"),
        tool_path(name = "strip", path = "strip.sh"),
    ]
    return cc_common.create_cc_toolchain_config_info(
        ctx = ctx,
        toolchain_identifier = "bexos-" + ctx.attr.cpu + "-" + ctx.attr.abi,
        host_system_name = "local",
        target_system_name = ctx.attr.cpu + "-unknown-" + ctx.attr.abi,
        target_cpu = ctx.attr.cpu,
        target_libc = "none",
        compiler = "clang",
        abi_version = "unknown",
        abi_libc_version = "none",
        tool_paths = tool_paths,
        cxx_builtin_include_directories = [
            "%workspace%/" + resources[0].path + "/include",
            "%workspace%/build/toolchains/cc/include",
        ],
    )

cc_toolchain_config = rule(
    implementation = _impl,
    attrs = {
        "cpu": attr.string(default = "aarch64"),
        "abi": attr.string(default = "linux-gnu"),
        "clang_wrapper": attr.string(default = "clang.sh"),
        "_headers": attr.label(default = ":llvm_headers", cfg = "exec"),
    },
    provides = [CcToolchainConfigInfo],
)
