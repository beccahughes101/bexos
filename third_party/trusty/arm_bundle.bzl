"""Explicit ARM boot bundles and authenticated live-replacement fixtures."""

def arm_firmware_image(name, firmware, candidate, config, generation, variant = "standard"):
    native.genrule(
        name = name + "_built_image",
        tags = ["manual"],
        srcs = [firmware],
        outs = [name + "/image.bin"],
        tools = [":image_bundle_tool"],
        cmd = "$(location :image_bundle_tool) --variant " + variant + " pack $@ $(locations " + firmware + ")",
    )
    native.genrule(
        name = name + "_decorated_candidate",
        tags = ["manual"],
        srcs = [candidate, config],
        outs = [name + "/candidate.elf"],
        tools = [":decorate_arm_candidate.py"],
        cmd = "$(location :decorate_arm_candidate.py) " +
              "$(location " + candidate + ") $(location " + config + ") " +
              str(generation) + " $@",
    )
    native.genrule(
        name = name + "_replacement",
        tags = ["manual"],
        srcs = [":" + name + "_decorated_candidate", "//device/virtual/qemu/base:keys/avb_dev_rsa_private.pem"],
        outs = [name + ".fw", name + ".avbpubkey"],
        tools = ["//tools/image:vbmeta"],
        cmd = "$(location //tools/image:vbmeta) --firmware " +
              "$(location //device/virtual/qemu/base:keys/avb_dev_rsa_private.pem) " +
              "$(location " + name + ".fw) $(location " + name + ".avbpubkey) " +
              str(generation) + " aarch64 trusty $(location :" + name + "_decorated_candidate)",
    )
