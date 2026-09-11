"""Execution-host dependencies shared by every Trusty firmware variant."""

_HOSTS = {
    "linux_x86_64": ("linux", "x86_64", "x86_64-unknown-linux-gnu"),
    "linux_aarch64": ("linux", "aarch64", "aarch64-unknown-linux-gnu"),
    "darwin_aarch64": ("osx", "aarch64", "aarch64-apple-darwin"),
}

def trusty_host_tools():
    # As a genrule tool this target is configured for execution, even when
    # the firmware itself is requested under a bare-metal guest platform.
    _host_environment(
        name = "host_environment",
        sdk_marker = select({
            "@platforms//os:osx": "@trusty_macos_sdk//:MacOSX.sdk/usr/include/stdio.h",
            "//conditions:default": None,
        }),
        sdk_files = select({
            "@platforms//os:osx": "@trusty_macos_sdk//:toolchain_files",
            "//conditions:default": None,
        }),
    )
    for name, (os, cpu, _) in _HOSTS.items():
        native.config_setting(
            name = name,
            constraint_values = ["@platforms//os:" + os, "@platforms//cpu:" + cpu],
        )
    native.filegroup(
        name = "host_rust_std",
        srcs = select({
            ":" + name: ["@trusty_rust_1_80_1//:rust_std-" + triple]
            for name, (_, _, triple) in _HOSTS.items()
        }),
    )
    native.alias(
        name = "host_rustfmt",
        actual = select({
            ":" + name: "@rustfmt_nightly-2026-07-16__" + triple + "_tools//:rustfmt"
            for name, (_, _, triple) in _HOSTS.items()
        }),
    )

def _host_environment_impl(ctx):
    marker = ctx.file.sdk_marker
    script = ctx.actions.declare_file(ctx.label.name + ".sh")
    ctx.actions.write(script, '#!/bin/sh\nTRUSTY_MACOS_SDK_MARKER="%s"\n' % (marker.path if marker else ""), is_executable = True)
    files = ctx.files.sdk_files + ([marker] if marker else [])
    return [DefaultInfo(executable = script, runfiles = ctx.runfiles(files = files))]

_host_environment = rule(
    implementation = _host_environment_impl,
    executable = True,
    attrs = {
        "sdk_marker": attr.label(allow_single_file = True),
        "sdk_files": attr.label(allow_files = True),
    },
)
