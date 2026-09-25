load("@rules_cc//cc:action_names.bzl", "ACTION_NAMES")
load("@rules_cc//cc:cc_toolchain_config_lib.bzl", "feature", "flag_group", "flag_set", "tool_path")
load("@rules_cc//cc/common:cc_common.bzl", "cc_common")
load("@rules_cc//cc/toolchains:cc_toolchain_config_info.bzl", "CcToolchainConfigInfo")

def _impl(ctx):
    headers = ctx.attr._headers[DefaultInfo].files.to_list()
    if len(headers) != 1:
        fail("expected one LLVM builtin resource directory")
    compiler = "clang_" + ctx.attr.cpu + ".sh"
    tools = [
        tool_path(name = name, path = path)
        for name, path in [
            ("ar", "ar.sh"),
            ("cpp", compiler),
            ("gcc", compiler),
            ("gcov", "nm.sh"),
            ("ld", compiler),
            ("nm", "nm.sh"),
            ("objdump", "nm.sh"),
            ("strip", "nm.sh"),
        ]
    ]
    return cc_common.create_cc_toolchain_config_info(
        ctx = ctx,
        toolchain_identifier = "bexos-sdk-" + ctx.attr.cpu,
        host_system_name = "local",
        target_system_name = ctx.attr.cpu + "-bexos",
        target_cpu = ctx.attr.cpu,
        target_libc = "bexos",
        compiler = "clang",
        abi_version = "1",
        abi_libc_version = "1",
        tool_paths = tools,
        cxx_builtin_include_directories = [
            "%workspace%/" + headers[0].path + "/include",
            "%workspace%/platforms/../sysroot/" + ctx.attr.cpu + "/include",
        ],
        features = [feature(
            name = "bexos_compile_flags",
            enabled = True,
            flag_sets = [flag_set(
                actions = [ACTION_NAMES.c_compile, ACTION_NAMES.cpp_compile, ACTION_NAMES.assemble, ACTION_NAMES.preprocess_assemble],
                flag_groups = [flag_group(flags = ["-ffreestanding", "-fno-stack-protector"])],
            )],
        )],
    )

sdk_cc_toolchain_config = rule(
    implementation = _impl,
    attrs = {
        "cpu": attr.string(mandatory = True),
        "_headers": attr.label(default = ":llvm_headers", cfg = "exec"),
    },
    provides = [CcToolchainConfigInfo],
)
