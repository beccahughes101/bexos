def _sdk_archive_impl(ctx):
    args = ctx.actions.args()
    args.add("archive")
    args.add("--out", ctx.outputs.out)
    args.add("--root", "bexos-sdk")

    files = []
    seen = {}
    for src in ctx.files.srcs:
        short_path = src.short_path
        destination = short_path
        if destination.startswith(ctx.attr.strip_prefix):
            destination = destination[len(ctx.attr.strip_prefix):]
        if destination in seen:
            fail("duplicate SDK destination %s from %s and %s" % (destination, seen[destination], src.path))
        seen[destination] = src.path
        files.append((destination, src, False))
    for target, destination in ctx.attr.renamed_files.items():
        srcs = target[DefaultInfo].files.to_list()
        if len(srcs) != 1:
            fail("renamed SDK input %s must produce exactly one file" % target.label)
        if destination in seen:
            fail("duplicate SDK destination %s" % destination)
        seen[destination] = srcs[0].path
        files.append((destination, srcs[0], False))
    for target, destination in ctx.attr.tools.items():
        srcs = target[DefaultInfo].files.to_list()
        if len(srcs) != 1:
            fail("SDK tool %s must produce exactly one file" % target.label)
        if destination in seen:
            fail("duplicate SDK destination %s" % destination)
        seen[destination] = srcs[0].path
        files.append((destination, srcs[0], True))

    files = sorted(files, key = lambda item: item[0])
    for destination, src, executable in files:
        args.add("--file", "%s=%s%s" % (destination, src.path, ":executable" if executable else ""))

    ctx.actions.run(
        executable = ctx.executable._packager,
        inputs = [item[1] for item in files],
        outputs = [ctx.outputs.out],
        arguments = [args],
        mnemonic = "BexosSdkArchive",
        progress_message = "Packaging %s" % ctx.outputs.out.short_path,
    )
    return [DefaultInfo(files = depset([ctx.outputs.out]))]

sdk_archive = rule(
    implementation = _sdk_archive_impl,
    attrs = {
        "srcs": attr.label_list(allow_files = True),
        "renamed_files": attr.label_keyed_string_dict(allow_files = True),
        "tools": attr.label_keyed_string_dict(allow_files = True),
        "strip_prefix": attr.string(default = "sdk/export/"),
        "out": attr.output(mandatory = True),
        "_packager": attr.label(
            default = "//tools/sdk_packager:sdk_packager",
            executable = True,
            cfg = "exec",
        ),
    },
)

def _sdk_checksum_impl(ctx):
    args = ctx.actions.args()
    args.add("sha256")
    args.add("--input", ctx.file.src)
    args.add("--out", ctx.outputs.out)
    ctx.actions.run(
        executable = ctx.executable._packager,
        inputs = [ctx.file.src],
        outputs = [ctx.outputs.out],
        arguments = [args],
        mnemonic = "BexosSdkChecksum",
    )
    return [DefaultInfo(files = depset([ctx.outputs.out]))]

sdk_checksum = rule(
    implementation = _sdk_checksum_impl,
    attrs = {
        "src": attr.label(mandatory = True, allow_single_file = True),
        "out": attr.output(mandatory = True),
        "_packager": attr.label(
            default = "//tools/sdk_packager:sdk_packager",
            executable = True,
            cfg = "exec",
        ),
    },
)
