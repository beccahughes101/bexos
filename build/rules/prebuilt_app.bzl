load("//build/platforms:architecture.bzl", "guest_select")

BexosPrebuiltAppInfo = provider(
    doc = "A verified, passive out-of-tree BEX application package.",
    fields = {
        "archive": "Original signed .bex file",
        "manifest": "Verified package.bexmanifest extracted from the archive",
        "package_id": "Canonical manifest package name",
        "autoinstall": "Whether the system image installs the package at boot",
    },
)

def _single_file(target, description):
    files = target[DefaultInfo].files.to_list()
    if len(files) != 1:
        fail("%s must provide exactly one file, got %d" % (description, len(files)))
    return files[0]

def _prebuilt_app_impl(ctx):
    archive = _single_file(ctx.attr.archive, "archive")
    public_key = _single_file(ctx.attr.public_key, "public_key")
    manifest = ctx.actions.declare_file(ctx.label.name + ".bexmanifest")
    args = ctx.actions.args()
    args.add("extract-manifest")
    args.add("--archive", archive)
    args.add("--public-key", public_key)
    args.add("--package-id", ctx.attr.package_id)
    args.add("--architecture", ctx.attr.architecture)
    args.add("--maximum-abi-version", ctx.attr.maximum_abi_version)
    args.add("--out", manifest)
    ctx.actions.run(
        executable = ctx.executable._archive_tool,
        inputs = [archive, public_key],
        outputs = [manifest],
        arguments = [args],
        mnemonic = "VerifyBexosPrebuiltApp",
        progress_message = "Verifying out-of-tree app %s" % ctx.attr.package_id,
    )
    return [
        DefaultInfo(files = depset([archive, manifest])),
        BexosPrebuiltAppInfo(
            archive = archive,
            manifest = manifest,
            package_id = ctx.attr.package_id,
            autoinstall = ctx.attr.autoinstall,
        ),
    ]

_prebuilt_app = rule(
    implementation = _prebuilt_app_impl,
    attrs = {
        "archive": attr.label(mandatory = True, allow_files = [".bex"]),
        "package_id": attr.string(mandatory = True),
        "public_key": attr.label(mandatory = True, allow_files = True),
        "autoinstall": attr.bool(default = True),
        "architecture": attr.string(mandatory = True, values = ["AARCH64", "X86_64"]),
        "maximum_abi_version": attr.int(default = 1),
        "_archive_tool": attr.label(
            default = "//tools/app_archive:bex_archive",
            executable = True,
            cfg = "exec",
        ),
    },
)

def bexos_prebuilt_app(name, archive, package_id, public_key, autoinstall = True, visibility = None, tags = []):
    """Verifies and exposes a signed out-of-tree application to product assembly."""
    _prebuilt_app(
        name = name,
        archive = archive,
        package_id = package_id,
        public_key = public_key,
        autoinstall = autoinstall,
        architecture = guest_select("AARCH64", "X86_64"),
        visibility = visibility,
        tags = tags,
    )
