"""Compile QEMU launch policy from protobuf text using Bazel's protoc."""

def qemu_launch_config(name, src):
    native.genrule(
        name = name,
        srcs = [src, "//tools/qemu:launch.proto"],
        outs = [name + ".bin"],
        cmd = "$(location @protobuf//:protoc) --proto_path=. --encode=bexos.tools.qemu.LaunchConfig tools/qemu/launch.proto < $(location %s) > $@" % src,
        tools = ["@protobuf//:protoc"],
    )
