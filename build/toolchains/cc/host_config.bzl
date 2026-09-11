"""Native host C/C++ toolchain; guest toolchains retain registration precedence."""
load("@rules_cc//cc:cc_toolchain_config_lib.bzl", "artifact_name_pattern", "feature", "flag_group", "flag_set", "tool_path")
load("@rules_cc//cc:action_names.bzl", "ACTION_NAMES")
load("@rules_cc//cc/common:cc_common.bzl", "cc_common")
load("@rules_cc//cc/toolchains:cc_toolchain_config_info.bzl", "CcToolchainConfigInfo")

_COMPILE = [ACTION_NAMES.c_compile, ACTION_NAMES.cpp_compile, ACTION_NAMES.assemble, ACTION_NAMES.preprocess_assemble]
_LINK = [ACTION_NAMES.cpp_link_executable, ACTION_NAMES.cpp_link_dynamic_library, ACTION_NAMES.cpp_link_nodeps_dynamic_library]

def _impl(ctx):
    resources = ctx.attr._headers[DefaultInfo].files.to_list()
    flags = ["-fPIC"]
    includes = ["/usr/include", "/usr/local/include", "/usr/lib", "/usr/local/lib", "%workspace%/" + resources[0].path + "/include"]
    link_flags = []
    darwin_features = []
    if ctx.file.sdk_marker:
        sdk = ctx.file.sdk_marker.dirname[:-len("/usr/include")]
        flags += ["-isysroot", sdk]
        link_flags += ["-isysroot", sdk]
        includes += ["%workspace%/" + sdk]
        darwin_features = [
            feature(name = "shared_flag", flag_sets = [flag_set(
                actions = _LINK[1:],
                flag_groups = [flag_group(flags = ["-dynamiclib"])],
            )]),
            feature(name = "runtime_library_search_directories", flag_sets = [flag_set(
                actions = _LINK,
                flag_groups = [flag_group(
                    iterate_over = "runtime_library_search_directories",
                    flags = ["-Wl,-rpath,@loader_path/%{runtime_library_search_directories}"],
                )],
            )]),
            feature(name = "set_install_name", enabled = True, flag_sets = [flag_set(
                actions = _LINK[1:],
                flag_groups = [flag_group(
                    expand_if_available = "runtime_solib_name",
                    flags = ["-Wl,-install_name,@rpath/%{runtime_solib_name}"],
                )],
            )]),
        ]
    return cc_common.create_cc_toolchain_config_info(
        ctx = ctx,
        toolchain_identifier = "bexos-native-" + ctx.attr.cpu,
        host_system_name = "local",
        target_system_name = "local",
        target_cpu = ctx.attr.cpu,
        target_libc = "macosx" if ctx.file.sdk_marker else "local",
        artifact_name_patterns = [artifact_name_pattern(category_name = "dynamic_library", prefix = "lib", extension = ".dylib")] if ctx.file.sdk_marker else [],
        compiler = "clang",
        abi_version = "local",
        abi_libc_version = "local",
        tool_paths = [tool_path(name = name, path = path) for name, path in [
            ("gcc", "clang_host.sh"), ("cpp", "clang_host.sh"), ("ld", "clang_host.sh"),
            ("gcov", "clang_host.sh"), ("ar", "ar.sh"), ("nm", "nm.sh"),
            ("objdump", "nm.sh"), ("strip", "strip.sh"),
        ]],
        cxx_builtin_include_directories = includes,
        features = darwin_features + [
            feature(name = "supports_pic", enabled = True),
            feature(name = "native_compile_flags", enabled = True, flag_sets = [flag_set(actions = _COMPILE, flag_groups = [flag_group(flags = flags)])]),
            feature(name = "native_link_flags", enabled = True, flag_sets = [flag_set(actions = _LINK, flag_groups = [flag_group(flags = link_flags)])]) if link_flags else feature(name = "native_link_flags"),
        ],
    )

host_cc_config = rule(
    implementation = _impl,
    attrs = {
        "cpu": attr.string(mandatory = True),
        "sdk_marker": attr.label(allow_single_file = True),
        "_headers": attr.label(default = ":llvm_headers", cfg = "exec"),
    },
    provides = [CcToolchainConfigInfo],
)
