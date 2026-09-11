"""Rules for runtime-embedded, digest-selected Dioxus Pulley artifacts."""

def dioxus_precompiled(name, app):
    native.genrule(
        name = name + "_composed",
        srcs = [app, "//apps/dioxus_shared:dioxus_component"],
        outs = [name + ".wasm"],
        tools = ["//tools/wasm:compose_dioxus"],
        cmd = "$(execpath //tools/wasm:compose_dioxus) $(location " + app + ") $(location //apps/dioxus_shared:dioxus_component) $@",
    )
    native.genrule(
        name = name + "_precompiled",
        srcs = [":" + name + "_composed"],
        outs = [name + ".cwasm.packed", name + ".sha256"],
        tools = ["//tools/wasm:precompile"],
        cmd = "$(execpath //tools/wasm:precompile) $(location :" + name + "_composed) $(location " + name + ".cwasm.packed) $(location " + name + ".sha256)",
    )
    native.filegroup(name = name + "_code", srcs = [name + ".cwasm.packed"])
    native.filegroup(name = name + "_digest", srcs = [name + ".sha256"])
