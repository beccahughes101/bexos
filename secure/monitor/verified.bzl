"""Pinned x86 boot payloads and signed AVB metadata for the monitor verifier."""
load("//build/platforms:architecture.bzl", "architecture_files")

def verified_payload(name):
    for part, target in {
        "kernel": "//kernel:kernel",
        "bootfs": "//device/virtual/qemu/nongui:bootfs_image",
        "policy": "//device/virtual/qemu/nongui:platform_config_bin",
        "handoff": "//device/virtual/qemu/nongui:boot_handoff.bin",
    }.items():
        architecture_files(name = name + "_" + part, srcs = [target], architecture = "x86_64")
    for variant, generation, policy in [("", 2, "policy"), ("_wrong_policy", 2, "kernel"), ("_zero_generation", 0, "policy")]:
        native.genrule(
            name = name + variant + "_vbmeta",
            srcs = [":" + name + "_kernel", ":" + name + "_bootfs", ":" + name + "_policy", "//device/virtual/qemu/base:keys/avb_dev_rsa_private.pem"],
            outs = [name + variant + ".vbmeta", name + variant + ".avbpubkey"],
            tools = ["//tools/image:vbmeta"],
            cmd = "$(location //tools/image:vbmeta) $(location //device/virtual/qemu/base:keys/avb_dev_rsa_private.pem) " +
                "$(location " + name + variant + ".vbmeta) $(location " + name + variant + ".avbpubkey) " + str(generation) +
                " kernel=$(location :" + name + "_kernel) bootfs=$(location :" + name + "_bootfs) platform-policy=$(location :" + name + "_" + policy + ")",
        )
    for variant in ["architecture", "handoff", "bootfs", "evidence", "update"]:
        fixture = name + "_invalid_" + variant
        native.genrule(
            name = fixture,
            srcs = [":" + name + "_kernel", "tests/invalid_kernel.py"],
            outs = [fixture + ".elf"],
            cmd = "python3 -B $(location tests/invalid_kernel.py) $(location :" + name + "_kernel) $@ " + variant,
        )
        native.genrule(
            name = fixture + "_vbmeta",
            srcs = [":" + fixture, ":" + name + "_bootfs", ":" + name + "_policy", "//device/virtual/qemu/base:keys/avb_dev_rsa_private.pem"],
            outs = [fixture + ".vbmeta", fixture + ".avbpubkey"],
            tools = ["//tools/image:vbmeta"],
            cmd = "$(location //tools/image:vbmeta) $(location //device/virtual/qemu/base:keys/avb_dev_rsa_private.pem) " +
                "$(location " + fixture + ".vbmeta) $(location " + fixture + ".avbpubkey) 2 " +
                "kernel=$(location :" + fixture + ") bootfs=$(location :" + name + "_bootfs) platform-policy=$(location :" + name + "_policy)",
        )
