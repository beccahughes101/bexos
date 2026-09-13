"""Guest architecture is independent of the machine executing Bazel tools."""

def _architecture_impl(ctx):
    if ctx.build_setting_value not in ["aarch64", "x86_64"]:
        fail("guest architecture must be aarch64 or x86_64")
    return []

architecture = rule(implementation = _architecture_impl, build_setting = config.string(flag = True))

def _trusty_variant_impl(ctx):
    if ctx.build_setting_value not in ["standard", "acceptance"]:
        fail("Trusty variant must be standard or acceptance")
    return []

trusty_variant = rule(implementation = _trusty_variant_impl, build_setting = config.string(flag = True))

def guest_select(arm, x86):
    return select(
        {"//build/platforms:is_aarch64": arm, "//build/platforms:is_x86_64": x86},
        no_match_error = "guest architecture must be aarch64 or x86_64",
    )

def guest_platform(tier):
    return guest_select("//build/platforms:" + tier + "_aarch64", "//build/platforms:" + tier + "_x86_64")


def _guest_transition_impl(settings, attr):
    return {"//build/platforms:guest_arch": attr.architecture}

guest_transition = transition(
    implementation = _guest_transition_impl,
    inputs = [],
    outputs = ["//build/platforms:guest_arch"],
)

def _guest_test_transition_impl(settings, attr):
    return {
        "//build/platforms:guest_arch": attr.architecture,
        "//build/platforms:trusty_variant": attr.trusty_variant,
        "//services/scened:legacy_standalone": attr.scened_standalone,
        "//data/locale:locales": attr.locales or settings["//data/locale:locales"],
    }

guest_test_transition = transition(
    implementation = _guest_test_transition_impl,
    inputs = ["//data/locale:locales"],
    outputs = ["//build/platforms:guest_arch", "//build/platforms:trusty_variant", "//services/scened:legacy_standalone", "//data/locale:locales"],
)

def _architecture_files_impl(ctx):
    return [DefaultInfo(files = depset(transitive = [target[DefaultInfo].files for target in ctx.attr.srcs]))]

architecture_files = rule(
    implementation = _architecture_files_impl,
    attrs = {
        "srcs": attr.label_list(cfg = guest_transition, allow_files = True),
        "architecture": attr.string(mandatory = True, values = ["aarch64", "x86_64"]),
        "_allowlist_function_transition": attr.label(default = "@bazel_tools//tools/allowlists/function_transition_allowlist"),
    },
)

def _bool_flag_impl(ctx):
    return []

bool_flag = rule(implementation = _bool_flag_impl, build_setting = config.bool(flag = True))
