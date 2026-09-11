"""Explicit architecture-specific saved firmware, independent of BexOS boot."""
def x86_firmware_bundle(name, firmware, boot, acceptance = False):
    flags = ["--architecture", "x86_64"] + (["--variant", "authmgr_acceptance"] if acceptance else [])
    native.genrule(
        name = name + "_built_image",
        tags = ["manual"],
        srcs = [firmware, boot],
        outs = ["bundle_" + name + "/image.bin"],
        tools = [":image_bundle_tool"],
        cmd = "$(location :image_bundle_tool) " + " ".join(flags) + " pack $@ $(locations %s) $(location %s)" % (firmware, boot),
    )
    native.sh_binary(
        name = "refresh_" + name + "_image",
        tags = ["manual"],
        srcs = ["image_bundle.sh"],
        data = ["image_bundle.py", ":" + name + "_built_image"],
        args = flags + ["refresh", "third_party/trusty/" + name + "_image.bin", "$(location :" + name + "_built_image)"],
    )
    native.filegroup(name = name + "_saved_image", srcs = native.glob([name + "_image.bin"], allow_empty = True))
    native.genrule(
        name = name + "_cached_firmware",
        srcs = [":" + name + "_saved_image"],
        outs = ["cached_" + name + "/" + file for file in ["lk.bin", "lk.elf", "boot.elf", "rpmb_dev", "RPMB_DATA", "build.prototxt"]],
        tools = [":image_bundle_tool"],
        cmd = "$(location :image_bundle_tool) " + " ".join(flags) + " extract $(@D)/cached_" + name + " $(SRCS)",
    )
    native.sh_binary(
        name = "run_" + name,
        srcs = ["run_x86.sh"],
        args = ["$(location :cached_" + name + "/boot.elf)", "$(location :cached_" + name + "/rpmb_dev)", "$(location :cached_" + name + "/RPMB_DATA)"],
        data = ["run_x86.py"] + [":cached_" + name + "/" + artifact for artifact in ["boot.elf", "rpmb_dev", "RPMB_DATA"]],
    )
    native.sh_test(
        name = name + "_test" if acceptance else name + "_boot_test",
        srcs = ["run_x86.sh"],
        args = ["$(location :cached_" + name + "/boot.elf)", "$(location :cached_" + name + "/rpmb_dev)", "$(location :cached_" + name + "/RPMB_DATA)", "--acceptance" if acceptance else "--smoke"],
        data = ["run_x86.py"] + [":cached_" + name + "/" + artifact for artifact in ["boot.elf", "rpmb_dev", "RPMB_DATA"]],
        tags = ["requires-qemu", "exclusive", "local", "no-sandbox"],
        timeout = "eternal" if acceptance else "long",
    )
