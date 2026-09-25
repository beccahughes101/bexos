load("//build/platforms:architecture.bzl", "guest_select")
load("//build/rules:prebuilt_app.bzl", "BexosPrebuiltAppInfo")

def _one_file(target, description):
    files = target[DefaultInfo].files.to_list()
    if len(files) != 1:
        fail("%s must provide exactly one file" % description)
    return files[0]

def _product_assembly_impl(ctx):
    args = ctx.actions.args()
    args.add("product")
    args.add("--architecture", ctx.attr.architecture)
    args.add("--product-bin", ctx.file.product)
    inputs = [ctx.file.product]
    for bundle in ctx.attr.bundles:
        bundle_file = _one_file(bundle, "assembly bundle")
        inputs.append(bundle_file)
        args.add("--bundle-bin", bundle_file)
    for target, label in ctx.attr.manifests.items():
        manifest = _one_file(target, "package manifest")
        inputs.append(manifest)
        args.add("--manifest-bin", "%s=%s" % (label, manifest.path))
    seen_prebuilt = {}
    native_grants = {grant: True for grant in ctx.attr.native_runner_grants}
    driver_grants = {grant: True for grant in ctx.attr.driver_grants}
    for target in ctx.attr.prebuilt_apps:
        info = target[BexosPrebuiltAppInfo]
        if info.package_id in seen_prebuilt:
            fail("duplicate prebuilt package id %s" % info.package_id)
        seen_prebuilt[info.package_id] = True
        grant = "%s@%s" % (info.package_id, info.signer_id)
        if info.component_type in ["service", "driver"] and grant not in native_grants:
            fail("external native component %s is missing exact runner-policy grant %s" % (info.package_id, grant))
        if info.component_type == "driver" and grant not in driver_grants:
            fail("external driver %s is missing exact driver-policy grant %s" % (info.package_id, grant))
        inputs.append(info.manifest)
        label = "prebuilt://%s" % info.package_id
        if info.signer_id:
            args.add("--prebuilt-component", "%s|%s|%s|%s|%s" % (
                label,
                info.placement,
                info.component_type,
                info.signer_id,
                info.manifest.path,
            ))
        else:
            args.add("--prebuilt-manifest-bin", "%s=%s" % (label, info.manifest.path))
    args.add("--out-index", ctx.outputs.index)
    args.add("--out-bootfs-labels", ctx.outputs.bootfs_labels)
    args.add("--out-system-image-labels", ctx.outputs.system_image_labels)
    ctx.actions.run(
        executable = ctx.executable._assembly,
        inputs = depset(inputs),
        outputs = [ctx.outputs.index, ctx.outputs.bootfs_labels, ctx.outputs.system_image_labels],
        arguments = [args],
        mnemonic = "BexosProductAssembly",
    )
    return [DefaultInfo(files = depset([ctx.outputs.index, ctx.outputs.bootfs_labels, ctx.outputs.system_image_labels]))]

_product_assembly = rule(
    implementation = _product_assembly_impl,
    attrs = {
        "product": attr.label(mandatory = True, allow_single_file = True),
        "bundles": attr.label_list(allow_files = True),
        "manifests": attr.label_keyed_string_dict(allow_files = True),
        "prebuilt_apps": attr.label_list(providers = [BexosPrebuiltAppInfo]),
        "native_runner_grants": attr.string_list(),
        "driver_grants": attr.string_list(),
        "architecture": attr.string(mandatory = True, values = ["aarch64", "x86_64"]),
        "index": attr.output(mandatory = True),
        "bootfs_labels": attr.output(mandatory = True),
        "system_image_labels": attr.output(mandatory = True),
        "_assembly": attr.label(default = "//tools/assembly:bexos_assembly", executable = True, cfg = "exec"),
    },
)

def _system_image_impl(ctx):
    args = ctx.actions.args()
    args.add("system-image")
    args.add("--base", ctx.file.base)
    inputs = [ctx.file.base]
    seen = {}
    for target in ctx.attr.prebuilt_apps:
        info = target[BexosPrebuiltAppInfo]
        if info.placement != "SYSTEM_IMAGE":
            continue
        if info.package_id in seen:
            fail("duplicate prebuilt package id %s" % info.package_id)
        seen[info.package_id] = True
        inputs.extend([info.archive, info.manifest])
        args.add("--package", "%s=%s" % (info.package_id, "true" if info.autoinstall else "false"))
    args.add("--out", ctx.outputs.out)
    ctx.actions.run(
        executable = ctx.executable._assembly,
        inputs = depset(inputs),
        outputs = [ctx.outputs.out],
        arguments = [args],
        mnemonic = "BexosSystemImageManifest",
    )
    return [DefaultInfo(files = depset([ctx.outputs.out]))]

system_image_with_prebuilt_apps = rule(
    implementation = _system_image_impl,
    attrs = {
        "base": attr.label(mandatory = True, allow_single_file = True),
        "prebuilt_apps": attr.label_list(providers = [BexosPrebuiltAppInfo]),
        "out": attr.output(mandatory = True),
        "_assembly": attr.label(default = "//tools/assembly:bexos_assembly", executable = True, cfg = "exec"),
    },
)

def _proto_root_cmd():
    return "PROTOBUF_SRC=; for proto in $(locations @protobuf//:well_known_type_protos); do case $$proto in */google/protobuf/any.proto) PROTOBUF_SRC=$${proto%/google/protobuf/any.proto};; esac; done; "

def _encode_cmd(message, proto, src, out):
    return _proto_root_cmd() + "$(location @protobuf//:protoc) --proto_path=. --proto_path=$$PROTOBUF_SRC --encode=%s %s < $(location %s) > %s" % (
        message,
        proto,
        src,
        out,
    )

def assembly_input_bundle(name, src):
    native.genrule(
        name = name + "_bin",
        srcs = [
            src,
            "//idl:platform_config_proto_src",
            "//idl:app_manifest_proto_src",
            "@protobuf//:well_known_type_protos",
        ],
        outs = [name + ".aib.bin"],
        cmd = _encode_cmd(
            "bexos.platform.AssemblyInputBundle",
            "idl/bexos/platform/assembly.proto",
            src,
            "$@",
        ),
        tools = ["@protobuf//:protoc"],
    )

def component_config_override(name, src):
    native.genrule(
        name = name + "_bin",
        srcs = [
            src,
            "//idl:platform_config_proto_src",
            "//idl:app_manifest_proto_src",
            "@protobuf//:well_known_type_protos",
        ],
        outs = [name + ".override.bin"],
        cmd = _encode_cmd(
            "bexos.platform.ComponentConfigOverride",
            "idl/bexos/platform/assembly.proto",
            src,
            "$@",
        ),
        tools = ["@protobuf//:protoc"],
    )

def app_config(name, manifest, override = None):
    srcs = [manifest]
    args = "config --manifest-bin $(location %s) --out $@" % manifest
    if override:
        srcs.append(override)
        args += " --override-bin $(location %s)" % override
    native.genrule(
        name = name,
        srcs = srcs,
        outs = [name + ".bexconfig"],
        cmd = "$(location //tools/assembly:bexos_assembly) " + args,
        tools = ["//tools/assembly:bexos_assembly"],
    )

def product_app_config(name, manifest, product, package_id):
    native.genrule(
        name = name,
        srcs = [manifest, product],
        outs = [name + ".bexconfig"],
        cmd = "$(location //tools/assembly:bexos_assembly) config --manifest-bin $(location %s) --product-bin $(location %s) --package-id %s --out $@" % (
            manifest,
            product,
            package_id,
        ),
        tools = ["//tools/assembly:bexos_assembly"],
    )

def component_config_rust(name, manifest):
    native.genrule(
        name = name,
        srcs = [manifest],
        outs = [name + ".rs"],
        cmd = "$(location //tools/assembly:bexos_assembly) config-rust --manifest-bin $(location %s) --out $@" % manifest,
        tools = ["//tools/assembly:bexos_assembly"],
    )

def product_definition(name, src):
    """Encode a product definition for config consumers without assembling it."""
    native.genrule(
        name = name + "_bin",
        srcs = [
            src,
            "//idl:platform_config_proto_src",
            "//idl:app_manifest_proto_src",
            "@protobuf//:well_known_type_protos",
        ],
        outs = [name + ".product.bin"],
        cmd = _encode_cmd(
            "bexos.platform.ProductDefinition",
            "idl/bexos/platform/assembly.proto",
            src,
            "$@",
        ),
        tools = ["@protobuf//:protoc"],
    )

def bexos_product(name, src, bundles, manifests, prebuilt_apps = [], native_runner_grants = [], driver_grants = []):
    product_definition(name = name, src = src)

    _product_assembly(
        name = name,
        product = ":" + name + "_bin",
        bundles = bundles,
        manifests = {manifest: label for label, manifest in manifests.items()},
        prebuilt_apps = prebuilt_apps,
        native_runner_grants = native_runner_grants,
        driver_grants = driver_grants,
        architecture = guest_select("aarch64", "x86_64"),
        index = name + ".assembly",
        bootfs_labels = name + ".bootfs.labels",
        system_image_labels = name + ".system_image.labels",
    )

def starlark_product(name, src, loads, entry, bundles, manifests, prebuilt_apps = [], native_runner_grants = [], driver_grants = []):
    definition = name + "_definition"
    load_args = "".join([
        " --load %s=$(location %s)" % (label, target)
        for label, target in sorted(loads.items())
    ])
    native.genrule(
        name = definition + "_text",
        srcs = [src] + loads.values(),
        outs = [definition + ".prototxt"],
        cmd = "$(location //tools/config_compiler:config_compiler) --source $(location %s) --entry %s%s --out $@" % (
            src,
            entry,
            load_args,
        ),
        tools = ["//tools/config_compiler:config_compiler"],
    )
    native.genrule(
        name = definition + "_bin",
        srcs = [
            ":" + definition + "_text",
            "//idl:platform_config_proto_src",
            "//idl:app_manifest_proto_src",
            "@protobuf//:well_known_type_protos",
        ],
        outs = [definition + ".product.bin"],
        cmd = _encode_cmd(
            "bexos.platform.ProductDefinition",
            "idl/bexos/platform/assembly.proto",
            ":" + definition + "_text",
            "$@",
        ),
        tools = ["@protobuf//:protoc"],
    )

    _product_assembly(
        name = name,
        product = ":" + definition + "_bin",
        bundles = bundles,
        manifests = {manifest: label for label, manifest in manifests.items()},
        prebuilt_apps = prebuilt_apps,
        native_runner_grants = native_runner_grants,
        driver_grants = driver_grants,
        architecture = guest_select("aarch64", "x86_64"),
        index = name + ".assembly",
        bootfs_labels = name + ".bootfs.labels",
        system_image_labels = name + ".system_image.labels",
    )

def product_app_config_policy(name, manifest, product, package_id):
    native.genrule(
        name = name,
        srcs = [manifest, product],
        outs = [name + ".bexpolicy"],
        cmd = "$(location //tools/assembly:bexos_assembly) config-policy --manifest-bin $(location %s) --product-bin $(location %s) --package-id %s --out $@" % (manifest, product, package_id),
        tools = ["//tools/assembly:bexos_assembly"],
    )
