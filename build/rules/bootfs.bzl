load("//build/rules:prebuilt_app.bzl", "BexosPrebuiltComponentInfo")

def _bootfs_with_prebuilt_components_impl(ctx):
    args = ctx.actions.args()
    args.add("--base", ctx.file.base)
    args.add("--out", ctx.outputs.out)
    inputs = [ctx.file.base]
    seen = {}
    for target in ctx.attr.prebuilt_components:
        info = target[BexosPrebuiltComponentInfo]
        if info.placement != "BOOTFS":
            continue
        if info.package_id in seen:
            fail("duplicate BOOTFS prebuilt package %s" % info.package_id)
        if info.package_tree == None:
            fail("BOOTFS prebuilt package %s has no verified package tree" % info.package_id)
        seen[info.package_id] = True
        inputs.extend([info.manifest, info.package_tree])
        args.add("--package-tree", "%s=%s" % (info.package_id, info.package_tree.path))
    ctx.actions.run(
        executable = ctx.executable._assemble_bootfs,
        inputs = depset(inputs),
        outputs = [ctx.outputs.out],
        arguments = [args],
        mnemonic = "BexosBootfsPrebuiltOverlay",
        progress_message = "Adding verified out-of-tree components to %s" % ctx.label,
    )
    return [DefaultInfo(files = depset([ctx.outputs.out]))]

bootfs_with_prebuilt_components = rule(
    implementation = _bootfs_with_prebuilt_components_impl,
    attrs = {
        "base": attr.label(mandatory = True, allow_single_file = True),
        "prebuilt_components": attr.label_list(providers = [BexosPrebuiltComponentInfo]),
        "out": attr.output(mandatory = True),
        "_assemble_bootfs": attr.label(
            default = "//tools/image:assemble_bootfs",
            executable = True,
            cfg = "exec",
        ),
    },
)
