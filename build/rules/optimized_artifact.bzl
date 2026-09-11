"""Select optimized platform executables and boot artifacts in fastbuild images."""
def _optimized_impl(_settings, _attr):
    return {"//command_line_option:compilation_mode": "opt"}

_optimized = transition(
    implementation = _optimized_impl,
    inputs = [],
    outputs = ["//command_line_option:compilation_mode"],
)

def _artifact_impl(ctx):
    target = ctx.attr.src[0]
    return [DefaultInfo(files = target[DefaultInfo].files)]

optimized_artifact = rule(
    implementation = _artifact_impl,
    attrs = {
        "src": attr.label(mandatory = True, cfg = _optimized, allow_files = True),
        "_allowlist_function_transition": attr.label(default = "@bazel_tools//tools/allowlists/function_transition_allowlist"),
    },
)

def _test_impl(ctx):
    target = ctx.attr.src[0][DefaultInfo]
    executable = ctx.actions.declare_file(ctx.label.name)
    ctx.actions.symlink(output = executable, target_file = target.files_to_run.executable, is_executable = True)
    runfiles = ctx.runfiles(files = [target.files_to_run.executable]).merge(target.default_runfiles)
    return [DefaultInfo(executable = executable, runfiles = runfiles)]

# Large real components exercise the Pulley compiler as well as guest execution.
# Keep compiler dependencies optimized even when the surrounding build is fastbuild.
optimized_test = rule(
    implementation = _test_impl,
    test = True,
    attrs = {
        "src": attr.label(mandatory = True, cfg = _optimized),
        "_allowlist_function_transition": attr.label(default = "@bazel_tools//tools/allowlists/function_transition_allowlist"),
    },
)
