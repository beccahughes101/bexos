load("//build/platforms:architecture.bzl", "guest_select")
load(":architecture_test.bzl", "architecture_test")
load("@rules_rust//rust:defs.bzl", "rust_test")

QEMU_HOST_RPMBD = "//third_party/trusty:host_rpmb_dev"

QEMU_BOOT_ARTIFACTS = {
    "kernel": "//kernel:kernel_bin",
    "disk": "//device/virtual/qemu/nongui:qemu_nvme_gpt.img",
    "bootfs": "//device/virtual/qemu/nongui:bootfs.img",
    "handoff": "//device/virtual/qemu/nongui:boot_handoff.bin",
    "evidence": "//device/virtual/qemu/nongui:boot_evidence.bin",
    "vbmeta": "//device/virtual/qemu/nongui:vbmeta.img",
    "avb_public_key": "//device/virtual/qemu/nongui:avb_dev_public_key.bin",
    "layout": "//device/virtual/qemu/nongui:boot_layout.sh",
    "inspector": "//tools/image:bexfs_image",
    "key": "//device/virtual/qemu/base:qemu_bexfs_test.key",
    # Execute the helper for this Bazel host. Firmware snapshots retain their
    # authenticated RPMB template, but their bundled helper belongs to the host
    # that produced the snapshot and must not cross a CI/job or workstation ABI
    # boundary.
    "rpmbd": QEMU_HOST_RPMBD,
    "rpmb_template": "//third_party/trusty:RPMB_DATA",
}

QEMU_BOOT_DATA = QEMU_BOOT_ARTIFACTS

QEMU_SECURE_FIRMWARE = {
    "bl1": "//third_party/trusty:bl1.bin",
    "bl2": "//third_party/trusty:bl2.bin",
    "bl31": "//third_party/trusty:bl31.bin",
    "bl32": "//third_party/trusty:lk.bin",
    "bl33": "//third_party/trusty:bl33.bin",
    "certificates": "//third_party/trusty:trusted_boot_certificates",
}

QEMU_TEST_TAGS = [
    "exclusive",
    "local",
    "no-sandbox",
    "requires-qemu",
]

def qemu_e2e_test(
        name,
        src,
        crate_name,
        deps,
        boot_data = QEMU_BOOT_ARTIFACTS,
        extra_data = [],
        extra_srcs = [],
        x86_extra_data = None,
        extra_args = [],
        x86_extra_args = None,
        qemu_args = [],
        x86_qemu_args = None,
        env = {},
        x86_env_overrides = {},
        tags = [],
        timeout = "eternal",
        secure_firmware = QEMU_SECURE_FIRMWARE,
        architectures = ["aarch64", "x86_64"],
        development = False,
        trusty_variant = "standard"):
    if native.existing_rule("declared_tests") != None:
        fail("qemu_suites() must follow every QEMU scenario in this package")
    test_env = {
        # Let the harness, not Bazel's target-size default, own QEMU boot and
        # request deadlines. Individual targets may override this value.
        "BEXOS_QEMU_TIMEOUT_SECONDS": "600",
        "BEXOS_QEMU_KERNEL": "$(rootpath %s)" % boot_data["kernel"],
        "BEXOS_QEMU_DISK": "$(rootpath %s)" % boot_data["disk"],
        "BEXOS_QEMU_BOOTFS": "$(rootpath %s)" % boot_data["bootfs"],
        "BEXOS_QEMU_HANDOFF": "$(rootpath %s)" % boot_data["handoff"],
        "BEXOS_QEMU_EVIDENCE": "$(rootpath %s)" % boot_data["evidence"],
        "BEXOS_QEMU_VBMETA": "$(rootpath %s)" % boot_data["vbmeta"],
        "BEXOS_QEMU_AVB_PUBLIC_KEY": "$(rootpath %s)" % boot_data["avb_public_key"],
        "BEXOS_QEMU_LAYOUT": "$(rootpath %s)" % boot_data["layout"],
        "BEXOS_QEMU_INSPECTOR": "$(rootpath %s)" % boot_data["inspector"],
        "BEXOS_QEMU_KEY": "$(rootpath %s)" % boot_data["key"],
        "BEXOS_QEMU_RPMBD": "$(rootpath %s)" % QEMU_HOST_RPMBD,
        "BEXOS_QEMU_RPMB_TEMPLATE": "$(rootpath %s)" % boot_data["rpmb_template"],
    }
    test_data = [
        boot_data["kernel"],
        boot_data["disk"],
        boot_data["bootfs"],
        boot_data["handoff"],
        boot_data["evidence"],
        boot_data["vbmeta"],
        boot_data["avb_public_key"],
        boot_data["layout"],
        boot_data["inspector"],
        boot_data["key"],
        boot_data["rpmb_template"],
    ]
    common_data = list(test_data)
    test_data += list(extra_data)
    if extra_args:
        test_env["BEXOS_QEMU_TEST_ARGS"] = " ".join(extra_args)
    if qemu_args:
        test_env["BEXOS_QEMU_EXTRA_ARGS"] = " ".join(qemu_args)
    arm_env = dict(test_env, BEXOS_QEMU_ARCH = "aarch64")
    arm_data = list(test_data)
    if secure_firmware:
        arm_env["BEXOS_QEMU_SECURE_BL1"] = "$(rootpath %s)" % secure_firmware["bl1"]
        arm_env["BEXOS_QEMU_SECURE_BL2"] = "$(rootpath %s)" % secure_firmware["bl2"]
        arm_env["BEXOS_QEMU_SECURE_BL31"] = "$(rootpath %s)" % secure_firmware["bl31"]
        arm_env["BEXOS_QEMU_SECURE_BL32"] = "$(rootpath %s)" % secure_firmware["bl32"]
        arm_env["BEXOS_QEMU_SECURE_BL33"] = "$(rootpath %s)" % secure_firmware["bl33"]
        arm_data.extend(secure_firmware.values())
    arm_env.update(env)
    x86_env = dict(test_env, BEXOS_QEMU_ARCH = "x86_64", BEXOS_QEMU_BOOT_IMAGE = "$(rootpath //device/virtual/qemu/nongui:x86_64_boot_image)")
    # The integrated monitor and normal-world provider consume the same saved
    # firmware bundle and authenticated persistent RPMB image.
    x86_env["BEXOS_QEMU_RPMBD"] = "$(rootpath %s)" % QEMU_HOST_RPMBD
    x86_env["BEXOS_QEMU_RPMB_TEMPLATE"] = "$(rootpath //third_party/trusty:cached_x86_64/RPMB_DATA)"
    if x86_extra_args != None:
        x86_env["BEXOS_QEMU_TEST_ARGS"] = " ".join(x86_extra_args)
    if x86_qemu_args != None:
        x86_env["BEXOS_QEMU_EXTRA_ARGS"] = " ".join(x86_qemu_args)
    if development:
        x86_env["BEXOS_QEMU_DEVELOPMENT"] = "1"
    x86_env.update(env)
    x86_base_data = test_data if x86_extra_data == None else common_data + x86_extra_data
    x86_data = [label for label in x86_base_data if label != boot_data["rpmb_template"]] + ["//device/virtual/qemu/nongui:x86_64_boot_image", "//third_party/trusty:cached_x86_64/RPMB_DATA"]
    if not development:
        x86_env.pop("BEXOS_QEMU_BOOT_IMAGE")
        secure_inputs = {
            "BEXOS_QEMU_RPMB_TEMPLATE": "//boot/efi:cached/RPMB_DATA",
            "BEXOS_QEMU_X86_OVMF_CODE": "//boot/efi:cached/OVMF_CODE.fd",
            "BEXOS_QEMU_X86_OVMF_VARS": "//boot/efi:cached/OVMF_VARS.fd",
            "BEXOS_QEMU_X86_EFI_LOADER": "//boot/efi:cached/loader.efi",
        }
        # The monitor loads ELF segments. Select its signed ELF descriptor only
        # for the standard product; explicit scenario inputs remain explicit.
        for key, default, replacement in [
            ("kernel", QEMU_BOOT_ARTIFACTS["kernel"], "//secure/monitor:product_payload_kernel"),
            ("vbmeta", QEMU_BOOT_ARTIFACTS["vbmeta"], "//secure/monitor:product_payload.vbmeta"),
        ]:
            if boot_data[key] == default:
                secure_inputs["BEXOS_QEMU_" + key.upper()] = replacement
        for key, label in secure_inputs.items():
            x86_env[key] = "$(rootpath %s)" % label
            x86_data.append(label)
    x86_env.update(x86_env_overrides)

    kwargs = {}
    if timeout:
        kwargs["timeout"] = timeout

    rust_test(
        name = name + "_implementation",
        srcs = [src] + extra_srcs,
        crate_root = src,
        crate_name = crate_name,
        edition = "2024",
        deps = deps,
        data = [QEMU_HOST_RPMBD] + guest_select(arm_data, x86_data),
        env = guest_select(arm_env, x86_env),
        tags = QEMU_TEST_TAGS + tags + ["manual"],
        use_libtest_harness = False,
        **kwargs
    )

    for arch in architectures:
        architecture_test(
            name = name + "_" + arch,
            test = ":" + name + "_implementation",
            architecture = arch,
            trusty_variant = trusty_variant if arch == "x86_64" else "standard",
            tags = QEMU_TEST_TAGS + tags + ["guest-" + arch, "qemu-development" if development else "qemu-integrated"],
            **kwargs
        )
    native.alias(name = name, actual = ":" + name + "_" + architectures[0] if len(architectures) == 1 else guest_select(":" + name + "_aarch64", ":" + name + "_x86_64"), tags = ["manual"])

# Deliberately separate from the integrated Trusty acceptance fixtures.
QEMU_DEVELOPMENT_BOOT_DATA = dict(QEMU_BOOT_ARTIFACTS, **{
    "disk": "//device/virtual/qemu/nongui:qemu_nvme_development_gpt.img",
    "bootfs": "//device/virtual/qemu/nongui:bootfs.emulated.img",
    "handoff": "//device/virtual/qemu/nongui:boot_handoff.emulated.bin",
    "evidence": "//device/virtual/qemu/nongui:boot_evidence.emulated.bin",
    "vbmeta": "//device/virtual/qemu/nongui:vbmeta.emulated.img",
    "avb_public_key": "//device/virtual/qemu/nongui:avb_dev_public_key.emulated.bin",
    "layout": "//device/virtual/qemu/nongui:boot_layout.emulated.sh",
})

def qemu_development_e2e_test(name, boot_data = QEMU_DEVELOPMENT_BOOT_DATA, **kwargs):
    qemu_e2e_test(name = name, boot_data = boot_data, architectures = ["x86_64"], development = True, secure_firmware = None, **kwargs)
