"""Transition complete test closures while leaving executable tests host-native."""
load("//build/platforms:architecture.bzl", "guest_test_transition")

def _architecture_test_impl(ctx):
    target = ctx.attr.test[0]
    info = target[DefaultInfo]
    executable = ctx.actions.declare_file(ctx.label.name)
    ctx.actions.symlink(output = executable, target_file = info.files_to_run.executable, is_executable = True)
    result = [DefaultInfo(executable = executable, runfiles = ctx.runfiles(files = [info.files_to_run.executable]).merge(info.default_runfiles))]
    if RunEnvironmentInfo in target:
        environment = target[RunEnvironmentInfo]
        result.append(RunEnvironmentInfo(environment = environment.environment, inherited_environment = environment.inherited_environment))
    return result

architecture_test = rule(
    implementation = _architecture_test_impl,
    test = True,
    attrs = {
        "test": attr.label(mandatory = True, cfg = guest_test_transition),
        "architecture": attr.string(mandatory = True, values = ["aarch64", "x86_64"]),
        "locales": attr.string_list(),
        "scened_standalone": attr.bool(default = False),
        "package_config": attr.string(default = ""),
        "trusty_variant": attr.string(default = "standard", values = ["standard", "acceptance"]),
        "_allowlist_function_transition": attr.label(default = "@bazel_tools//tools/allowlists/function_transition_allowlist"),
    },
)

def qemu_suites():
    rules = native.existing_rules()
    tests = {name: rule for name, rule in rules.items() if rule["kind"] == "architecture_test"}
    for arch in ["aarch64", "x86_64"]:
        native.test_suite(
            name = arch,
            tests = [":" + name for name, rule in tests.items() if name.endswith("_" + arch) and "qemu-integrated" in rule["tags"]],
            tags = ["guest-" + arch, "qemu-integrated"],
        )
    native.test_suite(name = "all_architectures", tests = [":aarch64", ":x86_64"])
    native.test_suite(
        name = "x86_64_development",
        tests = [":" + name for name, rule in tests.items() if "qemu-development" in rule["tags"]],
        tags = ["guest-x86_64", "qemu-development"],
    )
    # A genquery consumes this loading-time inventory without building guests.
    native.filegroup(name = "declared_tests", testonly = True, srcs = [":" + name for name, rule in rules.items() if rule["kind"].endswith("_test") and "requires-qemu" in rule.get("tags", [])])
