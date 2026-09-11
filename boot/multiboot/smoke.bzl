load(":boot_image.bzl", "multiboot_image")

def boot_smoke_targets():
    for mode in ["32", "64"]:
        native.genrule(
            name = "smoke" + mode,
            srcs = ["smoke.S", "smoke.ld"],
            outs = ["smoke" + mode + ".elf"],
            tools = ["@llvm//tools:clang", "@llvm//tools:ld.lld"],
            cmd = "$(location @llvm//tools:clang) --target=x86_64-unknown-none-elf " + ("-DLONG_MODE " if mode == "64" else "") + "-c $(location smoke.S) -o $(@D)/smoke" + mode + ".o && $(location @llvm//tools:ld.lld) -T $(location smoke.ld) $(@D)/smoke" + mode + ".o -o $@",
        )
        multiboot_image(name = "smoke" + mode + "_boot", kernel = ":smoke" + mode, trusty = mode == "64")
    native.sh_test(
        name = "handoff_test",
        srcs = ["smoke_test.sh"],
        args = ["$(location :smoke32_boot)", "$(location :smoke64_boot)"],
        data = ["smoke_test.py", ":smoke32_boot", ":smoke64_boot"],
        tags = ["requires-qemu", "local", "no-sandbox"],
    )
