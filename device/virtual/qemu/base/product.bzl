"""Compose a QEMU product without duplicating its assembly or boot chain."""
load("//build/platforms:architecture.bzl", "architecture_files", "guest_select")
load("//boot/multiboot:boot_image.bzl", "multiboot_image")
load(":assembly.bzl", "qemu_assembly")
load(":images.bzl", "qemu_images")
load(":launch.bzl", "QEMU_RUN_DATA", "QEMU_X86_SECURE_RUN_DATA", "qemu_launchers")

def qemu_product(name, graphics = False, input_fixture = False):
    qemu_assembly(product = name, graphics = graphics, input_fixture = input_fixture)

    qemu_images(graphics = graphics, input_fixture = input_fixture)

    native.alias(
        name = "launch_config",
        actual = "//device/virtual/qemu/base/graphics:launch_config" if graphics else "//device/virtual/qemu/base:launch_config",
    )
    qemu_launchers(name, ":launch_config")

    architecture_files(
        name = "virtual_aarch64",
        architecture = "aarch64",
        srcs = QEMU_RUN_DATA + [
            ":virtual_aarch64_product_assembly",
            ":platform_config_bin",
            ":system_image_bin",
            ":launch_config",
        ],
    )

    multiboot_image(
        name = "x86_64_boot_image",
        kernel = "//kernel:kernel",
        target_compatible_with = guest_select(["@platforms//:incompatible"], []),
    )

    architecture_files(
        name = "virtual_x86_64",
        architecture = "x86_64",
        srcs = QEMU_X86_SECURE_RUN_DATA + [":virtual_x86_64_product_assembly", ":device_bin", ":platform_config_bin", ":system_image_bin", ":launch_config"],
        tags = ["manual"],
    )

    native.alias(name = name, actual = guest_select(":virtual_aarch64", ":virtual_x86_64"))

    architecture_files(
        name = "virtual_x86_64_development",
        architecture = "x86_64",
        srcs = [":virtual_x86_64_development_product_assembly", ":vbmeta_emulated", ":x86_64_boot_image", ":bootfs_image_emulated", ":boot_handoff_emulated", ":qemu_nvme_development_disk_image", ":device_emulated_bin", ":platform_config_emulated_bin"],
        tags = ["manual"],
    )

    native.sh_test(
        name = "x86_64_development_boot_test",
        srcs = ["//device/virtual/qemu/base/x86_64:development_boot_test.sh"],
        args = ["$(location //device/virtual/qemu/base/x86_64:development_boot_test.py)", "$(location :x86_64_boot_image)", "$(location :bootfs.emulated.img)", "$(location :boot_handoff.emulated.bin)", "$(location :boot_evidence.emulated.bin)", "$(location :qemu_nvme_development_gpt.img)"],
        data = ["//device/virtual/qemu/base/x86_64:development_boot_test.py", ":x86_64_boot_image", ":bootfs.emulated.img", ":boot_handoff.emulated.bin", ":boot_evidence.emulated.bin", ":qemu_nvme_development_gpt.img"],
        target_compatible_with = guest_select(["@platforms//:incompatible"], []),
        tags = ["local", "manual"],
        timeout = "moderate",
    )
