load("//build/platforms:architecture.bzl", "guest_select")

BexosPrebuiltComponentInfo = provider(
    doc = "A verified, passive out-of-tree BEX component package.",
    fields = {
        "archive": "Original signed .bex file",
        "manifest": "Verified package.bexmanifest extracted from the archive",
        "package_tree": "Verified package contents for BOOTFS placement, or None",
        "package_id": "Canonical manifest package name",
        "autoinstall": "Whether the system image installs the package at boot",
        "placement": "SYSTEM_IMAGE or BOOTFS",
        "component_type": "application, service, or driver",
        "signer_id": "Verified product signing-root identity",
        "boot_wave": "Signed early-boot wave, or -1",
    },
)

BexosPrebuiltAppInfo = BexosPrebuiltComponentInfo

def _single_file(target, description):
    files = target[DefaultInfo].files.to_list()
    if len(files) != 1:
        fail("%s must provide exactly one file, got %d" % (description, len(files)))
    return files[0]

def _prebuilt_app_impl(ctx):
    archive = _single_file(ctx.attr.archive, "archive")
    public_key = _single_file(ctx.attr.public_key, "public_key")
    manifest = ctx.actions.declare_file(ctx.label.name + ".bexmanifest")
    package_tree = None
    args = ctx.actions.args()
    args.add("extract-manifest" if ctx.attr.legacy_app else "extract-component")
    args.add("--archive", archive)
    args.add("--public-key" if ctx.attr.legacy_app else "--signing-root", public_key)
    args.add("--package-id", ctx.attr.package_id)
    args.add("--architecture", ctx.attr.architecture)
    args.add("--maximum-abi-version", ctx.attr.maximum_abi_version)
    if ctx.attr.legacy_app:
        args.add("--out", manifest)
    else:
        package_tree = ctx.actions.declare_directory(ctx.label.name + ".package")
        args.add("--component-type", ctx.attr.component_type)
        args.add("--signer-id", ctx.attr.signer_id)
        args.add("--out-tree", package_tree.path)
        if ctx.attr.boot_wave >= 0:
            args.add("--boot-wave", ctx.attr.boot_wave)
    ctx.actions.run(
        executable = ctx.executable._archive_tool,
        inputs = [archive, public_key],
        outputs = [manifest] if ctx.attr.legacy_app else [package_tree],
        arguments = [args],
        mnemonic = "VerifyBexosPrebuiltApp",
        progress_message = "Verifying out-of-tree component %s" % ctx.attr.package_id,
    )
    if not ctx.attr.legacy_app:
        ctx.actions.run_shell(
            inputs = [package_tree],
            outputs = [manifest],
            command = "cp %s/package.bexmanifest %s" % (package_tree.path, manifest.path),
            mnemonic = "ExtractBexosPrebuiltManifest",
        )
    return [
        DefaultInfo(files = depset([archive, manifest] + ([package_tree] if package_tree else []))),
        BexosPrebuiltComponentInfo(
            archive = archive,
            manifest = manifest,
            package_tree = package_tree,
            package_id = ctx.attr.package_id,
            autoinstall = ctx.attr.autoinstall,
            placement = ctx.attr.placement,
            component_type = ctx.attr.component_type,
            signer_id = ctx.attr.signer_id,
            boot_wave = ctx.attr.boot_wave,
        ),
    ]

_prebuilt_app = rule(
    implementation = _prebuilt_app_impl,
    attrs = {
        "archive": attr.label(mandatory = True, allow_files = [".bex"]),
        "package_id": attr.string(mandatory = True),
        "public_key": attr.label(mandatory = True, allow_files = True),
        "autoinstall": attr.bool(default = True),
        "placement": attr.string(default = "SYSTEM_IMAGE", values = ["SYSTEM_IMAGE", "BOOTFS"]),
        "component_type": attr.string(default = "application", values = ["application", "service", "driver"]),
        "signer_id": attr.string(),
        "boot_wave": attr.int(default = -1),
        "legacy_app": attr.bool(default = False),
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
        legacy_app = True,
        architecture = guest_select("AARCH64", "X86_64"),
        visibility = visibility,
        tags = tags,
    )

def bexos_prebuilt_component(
        name,
        archive,
        package_id,
        signing_root,
        signer_id,
        component_type,
        placement = "SYSTEM_IMAGE",
        boot_wave = None,
        autoinstall = True,
        visibility = None,
        tags = []):
    """Verifies a signed external component for system-image or BOOTFS placement."""
    if placement == "BOOTFS" and boot_wave == None:
        fail("BOOTFS components require boot_wave")
    if placement == "SYSTEM_IMAGE" and boot_wave != None:
        fail("SYSTEM_IMAGE components must not set boot_wave")
    if placement == "BOOTFS" and autoinstall:
        fail("BOOTFS components cannot be autoinstall packages")
    _prebuilt_app(
        name = name,
        archive = archive,
        package_id = package_id,
        public_key = signing_root,
        signer_id = signer_id,
        component_type = component_type,
        placement = placement,
        boot_wave = -1 if boot_wave == None else boot_wave,
        autoinstall = autoinstall,
        architecture = guest_select("AARCH64", "X86_64"),
        visibility = visibility,
        tags = tags,
    )
