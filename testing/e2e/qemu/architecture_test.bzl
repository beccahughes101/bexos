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
    for name, rule in tests.items():
        tiers = [tag for tag in rule["tags"] if tag.startswith("e2e-tier-")]
        if len(tiers) != 1:
            fail("%s must have exactly one E2E tier tag, got %s" % (name, tiers))
    for arch in ["aarch64", "x86_64"]:
        integrated = {name: rule for name, rule in tests.items() if name.endswith("_" + arch) and "qemu-integrated" in rule["tags"]}
        presubmit = [":" + name for name, rule in integrated.items() if "e2e-tier-presubmit" in rule["tags"]]
        extended = [":" + name for name, rule in integrated.items() if "e2e-tier-extended" in rule["tags"]]
        focused = [":" + name for name, rule in integrated.items() if "e2e-tier-focused" in rule["tags"]]
        native.test_suite(
            name = arch,
            tests = presubmit + extended,
            tags = ["guest-" + arch, "qemu-integrated"],
        )
        native.test_suite(name = "presubmit_" + arch, tests = presubmit, tags = ["e2e-tier-presubmit", "guest-" + arch, "qemu-integrated"])
        native.test_suite(name = "extended_" + arch, tests = extended, tags = ["e2e-tier-extended", "guest-" + arch, "qemu-integrated"])
        native.test_suite(name = "focused_" + arch, tests = focused, tags = ["e2e-tier-focused", "guest-" + arch, "qemu-integrated"])
    native.test_suite(name = "all_architectures", tests = [":aarch64", ":x86_64"])
    development = {name: rule for name, rule in tests.items() if "qemu-development" in rule["tags"]}
    development_presubmit = [":" + name for name, rule in development.items() if "e2e-tier-presubmit" in rule["tags"]]
    development_extended = [":" + name for name, rule in development.items() if "e2e-tier-extended" in rule["tags"]]
    development_focused = [":" + name for name, rule in development.items() if "e2e-tier-focused" in rule["tags"]]
    native.test_suite(
        name = "x86_64_development",
        tests = development_presubmit + development_extended,
        tags = ["guest-x86_64", "qemu-development"],
    )
    native.test_suite(name = "presubmit_x86_64_development", tests = development_presubmit, tags = ["e2e-tier-presubmit", "guest-x86_64", "qemu-development"])
    native.test_suite(name = "extended_x86_64_development", tests = development_extended, tags = ["e2e-tier-extended", "guest-x86_64", "qemu-development"])
    native.test_suite(name = "focused_x86_64_development", tests = development_focused, tags = ["e2e-tier-focused", "guest-x86_64", "qemu-development"])
    # A genquery consumes this loading-time inventory without building guests.
    native.filegroup(name = "declared_tests", testonly = True, srcs = [":" + name for name, rule in rules.items() if rule["kind"].endswith("_test") and "requires-qemu" in rule.get("tags", [])])
