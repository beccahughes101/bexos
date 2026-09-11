"""Authenticated execution tests using validated saved Trusty firmware."""
load("//build/platforms:architecture.bzl", "architecture_files")
load(":monitor.bzl", "authenticated_monitor")

def trusty_execution(name, variant, monitor, acceptance = False, normal_world = False, boot_approval = False, secure_product = False, external_payload = False, resident_nucleus = False):
    if resident_nucleus and not secure_product:
        fail("resident firmware selection requires the secure product")
    firmware = "//third_party/trusty:cached_" + variant + "/"
    if secure_product:
        firmware = "//third_party/trusty:product_x86/"
    if secure_product:
        normal_world = True
        boot_approval = True
    external = ["//secure/monitor:" + part for part in [
        "product_payload_kernel", "product_payload_bootfs", "product_payload.vbmeta",
        "product_payload_wrong_policy.vbmeta", "product_payload_zero_generation.vbmeta",
    ]] if external_payload else []
    if normal_world:
        architecture_files(name = name + "_disk", srcs = ["//device/virtual/qemu/nongui:qemu_nvme_gpt.img" if secure_product else "//device/virtual/qemu/nongui:qemu_nvme_development_gpt.img"], architecture = "x86_64")
    authenticated_monitor(
        name = name,
        monitor = monitor,
        domain_banks = [(0x20000000, 0x10000000)] + ([(0x30000000, 0x30000000)] if normal_world else []) + ([(0x04000000, 0x200000), (0x60000000, 0x200000)] if resident_nucleus else []),
    )
    native.sh_test(
        name = name + "_test",
        srcs = ["test.sh"],
        args = ["$(location trusty_domain_test.py)", "$(location //third_party/ovmf:OVMF_CODE.fd)",
            "$(location :enrolled.fd)", "$(location :" + name + ")",
            "$(location //third_party/ovmf:OVMF_VARS.fd)",
            "$(location " + firmware + "rpmb_dev)",
            "$(location " + firmware + "RPMB_DATA)",
            "$(location " + firmware + "lk.elf)"] + (["--acceptance"] if acceptance else []) + (["--normal-world", "--disk", "$(location :" + name + "_disk)"] if normal_world else []) + (["--boot-approval"] if boot_approval else []) + (["--secure-product"] if secure_product else []) + (["--resident-nucleus"] if resident_nucleus else []) + (["--external-payload"] + ["$(location " + part + ")" for part in external] if external_payload else []),
        data = ["trusty_domain_test.py", "test.py", "qmp.py", "//third_party/ovmf:OVMF_CODE.fd",
            "//third_party/ovmf:OVMF_VARS.fd", ":enrolled.fd", ":" + name,
            firmware + "rpmb_dev", firmware + "RPMB_DATA", firmware + "lk.elf"] + ([":" + name + "_disk", "//third_party/trusty:x86_devices.py"] if normal_world else []) + external + (["payload_rejection.py"] if external_payload else []) + (["//third_party/trusty:firmware_recovery_test.py"] if resident_nucleus else []),
        tags = ["requires-qemu", "exclusive", "local", "no-sandbox"],
        timeout = "eternal" if secure_product or acceptance and normal_world else "long",
    )
