load("@rules_cc//cc:cc_toolchain_config_lib.bzl", "tool_path")
load("@rules_cc//cc/common:cc_common.bzl", "cc_common")
load("@rules_cc//cc/toolchains:cc_toolchain_config_info.bzl", "CcToolchainConfigInfo")

def _impl(ctx):
    tools = [
        tool_path(name = name, path = "false.sh")
        for name in ["ar", "cpp", "gcc", "gcov", "ld", "nm", "objdump", "strip"]
    ]
    return cc_common.create_cc_toolchain_config_info(
        ctx = ctx,
        toolchain_identifier = "bexos-sdk-" + ctx.attr.cpu,
        host_system_name = "local",
        target_system_name = ctx.attr.cpu + "-bexos",
        target_cpu = ctx.attr.cpu,
        target_libc = "none",
        compiler = "rust-lld",
        abi_version = "1",
        abi_libc_version = "none",
        tool_paths = tools,
    )

sdk_cc_toolchain_config = rule(
    implementation = _impl,
    attrs = {"cpu": attr.string(mandatory = True)},
    provides = [CcToolchainConfigInfo],
)
