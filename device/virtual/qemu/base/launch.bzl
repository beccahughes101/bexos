"""Developer launchers sharing the same secure firmware on both products."""
load("//build/platforms:architecture.bzl", "guest_select")
load("//build/rules:optimized_artifact.bzl", "optimized_artifact")

QEMU_RUN_DATA = [
    "//kernel:kernel_bin",
    ":qemu_nvme_gpt.img",
    ":bootfs.img",
    ":boot_handoff.bin",
    ":boot_evidence.bin",
    ":vbmeta.img",
    ":avb_dev_public_key.bin",
    ":boot_layout.sh",
    "//tools/image:bexfs_image",
    "//device/virtual/qemu/base:qemu_bexfs_test.key",
    "//third_party/trusty:rpmb_dev",
    "//third_party/trusty:RPMB_DATA",
    "//third_party/trusty:bl1.bin",
    "//third_party/trusty:bl2.bin",
    "//third_party/trusty:bl31.bin",
    "//third_party/trusty:lk.bin",
    "//third_party/trusty:trusted_boot_certificates",
    "//third_party/trusty:bl33.bin",
]

QEMU_X86_SECURE_RUN_DATA = [
    "//kernel:kernel", ":qemu_nvme_gpt.img", ":bootfs.img",
    ":boot_handoff.bin", ":boot_evidence.bin", ":vbmeta.elf.img",
    ":avb_dev_public_key.elf.bin", ":boot_layout.sh", "//tools/image:bexfs_image", "//device/virtual/qemu/base:qemu_bexfs_test.key",
    "//boot/efi:cached/rpmb_dev", "//boot/efi:cached/RPMB_DATA",
    "//boot/efi:cached/OVMF_CODE.fd", "//boot/efi:cached/OVMF_VARS.fd", "//boot/efi:cached/loader.efi",
]

QEMU_EMULATED_RUN_DATA = [
    ":emulated_kernel",
    ":qemu_nvme_development_gpt.img",
    ":bootfs.emulated.img",
    ":boot_handoff.emulated.bin",
    ":boot_evidence.emulated.bin",
    ":vbmeta.emulated.img",
    ":avb_dev_public_key.emulated.bin",
    ":boot_layout.emulated.sh",
    "//tools/image:bexfs_image",
    "//device/virtual/qemu/base:qemu_bexfs_test.key",
    "//third_party/trusty:rpmb_dev",
    "//third_party/trusty:RPMB_DATA",
]

def _locations(labels):
    return ["$(location " + label + ")" for label in labels]

def qemu_launchers(product, launch_config):
    # Developer boots must have the same bounded guest compilation and I/O costs
    # as the optimized QEMU regressions, even when Bazel itself uses fastbuild.
    # Keep the host launcher independent of the guest artifact configuration.
    optimized = {}
    for label in QEMU_RUN_DATA + QEMU_X86_SECURE_RUN_DATA:
        if label not in optimized:
            name = "run_artifact_" + str(len(optimized))
            optimized_artifact(name = name, src = label, tags = ["manual"])
            optimized[label] = ":" + name
    arm_data = [optimized[label] for label in QEMU_RUN_DATA]
    x86_data = [optimized[label] for label in QEMU_X86_SECURE_RUN_DATA]
    runner = "//tools/qemu:qemu_runner"
    common_args = [
        "$(location " + runner + ")",
        "--developer",
        "--product", product,
        "--launch-config", "$(location " + launch_config + ")",
    ] + guest_select(["--arch", "aarch64"], ["--arch", "x86_64"])
    firmware_args = guest_select(
        ["--secure-bl1"] + _locations([
            optimized["//third_party/trusty:bl1.bin"],
            optimized["//third_party/trusty:bl2.bin"],
            optimized["//third_party/trusty:bl31.bin"],
            optimized["//third_party/trusty:lk.bin"],
            optimized["//third_party/trusty:bl33.bin"],
        ]),
        ["--secure-x86"] + _locations(x86_data[12:]),
    )
    # The artifact protocol is ten boot/image files followed by the RPMB pair.
    artifact_args = guest_select(_locations(arm_data[:12]), _locations(x86_data[:12]))
    native.sh_binary(
        name = "run",
        srcs = ["//device/virtual/qemu/base:run_qemu.sh"],
        args = common_args + artifact_args + firmware_args,
        data = [runner, launch_config] + guest_select(arm_data, x86_data),
    )
    if product == "nongui":
        native.sh_binary(
            name = "run_emulated",
            srcs = ["//device/virtual/qemu/base:run_qemu.sh"],
            args = common_args + _locations(QEMU_EMULATED_RUN_DATA) + ["--development"] + guest_select([], ["--boot-image", "$(location :x86_64_boot_image)"]),
            data = [runner, launch_config] + QEMU_EMULATED_RUN_DATA + guest_select([], [":x86_64_boot_image"]),
        )
    native.sh_binary(
        name = "debugd",
        srcs = ["//device/virtual/qemu/base:debugd.sh"],
        args = ["$(location //tools/bexctl:bexctl)", product] + guest_select(
            ["aarch64"], ["x86_64"],
        ),
        data = ["//tools/bexctl:bexctl"],
    )
