load("//build/platforms:architecture.bzl", "guest_select")

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

def bexos_product(name, src, bundles, manifests):
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

    bundle_args = "".join([" --bundle-bin $(location %s)" % bundle for bundle in bundles])
    manifest_args = "".join([
        " --manifest-bin %s=$(location %s)" % (label, manifest)
        for label, manifest in sorted(manifests.items())
    ])
    native.genrule(
        name = name,
        srcs = [":" + name + "_bin"] + bundles + manifests.values(),
        outs = [
            name + ".assembly",
            name + ".bootfs.labels",
            name + ".system_image.labels",
        ],
        cmd = guest_select("$(location //tools/assembly:bexos_assembly) product --architecture aarch64 ", "$(location //tools/assembly:bexos_assembly) product --architecture x86_64 ") + "--product-bin $(location :%s_bin)%s%s --out-index $(location %s.assembly) --out-bootfs-labels $(location %s.bootfs.labels) --out-system-image-labels $(location %s.system_image.labels)" % (
            name,
            bundle_args,
            manifest_args,
            name,
            name,
            name,
        ),
        tools = ["//tools/assembly:bexos_assembly"],
    )

def starlark_product(name, src, loads, entry, bundles, manifests):
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

    bundle_args = "".join([" --bundle-bin $(location %s)" % bundle for bundle in bundles])
    manifest_args = "".join([
        " --manifest-bin %s=$(location %s)" % (label, manifest)
        for label, manifest in sorted(manifests.items())
    ])
    native.genrule(
        name = name,
        srcs = [":" + definition + "_bin"] + bundles + manifests.values(),
        outs = [
            name + ".assembly",
            name + ".bootfs.labels",
            name + ".system_image.labels",
        ],
        cmd = guest_select("$(location //tools/assembly:bexos_assembly) product --architecture aarch64 ", "$(location //tools/assembly:bexos_assembly) product --architecture x86_64 ") + "--product-bin $(location :%s_bin)%s%s --out-index $(location %s.assembly) --out-bootfs-labels $(location %s.bootfs.labels) --out-system-image-labels $(location %s.system_image.labels)" % (
            definition,
            bundle_args,
            manifest_args,
            name,
            name,
            name,
        ),
        tools = ["//tools/assembly:bexos_assembly"],
    )

def product_app_config_policy(name, manifest, product, package_id):
    native.genrule(
        name = name,
        srcs = [manifest, product],
        outs = [name + ".bexpolicy"],
        cmd = "$(location //tools/assembly:bexos_assembly) config-policy --manifest-bin $(location %s) --product-bin $(location %s) --package-id %s --out $@" % (manifest, product, package_id),
        tools = ["//tools/assembly:bexos_assembly"],
    )
