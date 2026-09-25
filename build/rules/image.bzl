load("//build/rules:prebuilt_app.bzl", "BexosPrebuiltAppInfo")

def _one_file(target, description):
    files = target[DefaultInfo].files.to_list()
    if len(files) != 1:
        fail("%s must provide exactly one file" % description)
    return files[0]

def _bexfs_image_impl(ctx):
    args = ctx.actions.args()
    args.add("--out", ctx.outputs.out)
    args.add("--size-bytes", ctx.attr.size_bytes)
    args.add("--label", ctx.attr.label)
    args.add("--volume-uuid", ctx.attr.volume_uuid)
    args.add("--key-file", ctx.file.key_file)
    inputs = [ctx.file.key_file]
    destinations = {}
    for target, destination in ctx.attr.entries.items():
        if destination in destinations:
            fail("duplicate BexFS destination %s" % destination)
        destinations[destination] = True
        source = _one_file(target, "BexFS entry")
        inputs.append(source)
        args.add("--entry", "%s=%s" % (destination, source.path))
    for target in ctx.attr.prebuilt_apps:
        info = target[BexosPrebuiltAppInfo]
        if info.placement != "SYSTEM_IMAGE":
            continue
        destination = "pkg/%s.bex" % info.package_id
        if destination in destinations:
            fail("duplicate BexFS destination %s" % destination)
        destinations[destination] = True
        inputs.extend([info.archive, info.manifest])
        args.add("--entry", "%s=%s" % (destination, info.archive.path))
    ctx.actions.run(
        executable = ctx.executable._bexfs_image,
        inputs = depset(inputs),
        outputs = [ctx.outputs.out],
        arguments = [args],
        mnemonic = "BexosBexfsImage",
    )
    return [DefaultInfo(files = depset([ctx.outputs.out]))]

bexfs_image_with_prebuilt_apps = rule(
    implementation = _bexfs_image_impl,
    attrs = {
        "entries": attr.label_keyed_string_dict(allow_files = True),
        "prebuilt_apps": attr.label_list(providers = [BexosPrebuiltAppInfo]),
        "size_bytes": attr.int(mandatory = True),
        "label": attr.string(mandatory = True),
        "volume_uuid": attr.string(mandatory = True),
        "key_file": attr.label(mandatory = True, allow_single_file = True),
        "out": attr.output(mandatory = True),
        "_bexfs_image": attr.label(default = "//tools/image:bexfs_image", executable = True, cfg = "exec"),
    },
)
