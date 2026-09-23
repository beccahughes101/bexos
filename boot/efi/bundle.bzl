"""Saved firmware boundaries: ordinary product builds never compile firmware."""
def integrated_firmware_bundle():
    names = ["loader.efi", "monitor.elf", "trusty.elf", "OVMF_CODE.fd", "OVMF_VARS.fd", "boot_root.avbpubkey", "rpmb_dev", "RPMB_DATA", "rollback_loader.efi", "revoked.fd", "development.pem", "trusty.replacement.fw", "trusty.successor.fw", "trusty.incompatible.fw", "trusty.fault.fw", "trusty.hang.fw", "hypervisor.replacement.fw", "trusty.candidate.elf", "trusty.successor.elf", "trusty.incompatible.elf", "trusty.fault.elf", "trusty.hang.elf", "monitor.policy.elf", "monitor.fault.elf", "monitor.hang.elf", "monitor.successor.elf", "hypervisor.fault.fw", "hypervisor.hang.fw", "hypervisor.successor.fw", "hypervisor.successor_fault.fw"]
    inputs = [
        ":authenticated_nucleus_product", "//secure/monitor:external_nucleus_product",
        "//third_party/trusty:product_x86/lk.elf", "//third_party/ovmf:OVMF_CODE.fd", ":enrolled.fd",
        "//secure/monitor:boot_root", "//third_party/trusty:product_x86/rpmb_dev", "//third_party/trusty:product_x86/RPMB_DATA",
        ":authenticated_rollback_provisioner", ":revoked.fd", ":development.pem",
        "//secure/monitor:trusty_recovery.fw", "//secure/monitor:trusty.successor.fw",
        "//secure/monitor:trusty.incompatible.fw", "//secure/monitor:trusty.fault.fw",
        "//secure/monitor:trusty.hang.fw", "//secure/monitor:monitor_policy_candidate.fw",
        "//third_party/trusty:built_x86_64_replacement/lk.elf", "//third_party/trusty:built_x86_64_generation3/lk.elf",
        "//third_party/trusty:built_x86_64_incompatible_state/lk.elf", "//third_party/trusty:built_x86_64_fault/lk.elf",
        "//third_party/trusty:built_x86_64_hang/lk.elf", "//secure/monitor:monitor_policy_candidate",
        "//secure/monitor:monitor_policy_fault", "//secure/monitor:monitor_policy_hang", "//secure/monitor:monitor_policy_successor",
        "//secure/monitor:monitor_policy_fault.fw", "//secure/monitor:monitor_policy_hang.fw", "//secure/monitor:monitor_policy_successor.fw",
        "//secure/monitor:monitor_successor_fault.fw",
    ]
    selected = ["//third_party/trusty:product_x86/lk.elf", "//third_party/ovmf:OVMF_CODE.fd", "//secure/monitor:boot_root"]
    scripts = ["bundle.py", "//tools/firmware:archive.py"]
    command = "python3 -B $(location bundle.py) $(location //tools/firmware:archive.py) "
    pack_suffix = " $@ " + " ".join(["$(location " + label + ")" for label in inputs])
    native.genrule(
        name = "built_firmware_bundle",
        tags = ["manual"],
        srcs = inputs + scripts,
        outs = ["built_firmware.bin"],
        cmd = select({"//build/platforms:trusty_acceptance": command + "pack acceptance" + pack_suffix, "//conditions:default": command + "pack standard" + pack_suffix}),
    )
    for value in ["standard", "acceptance"]:
        native.filegroup(name = value + "_saved_firmware", srcs = native.glob([value + "_firmware.bin"], allow_empty = True))
    saved = select({"//build/platforms:trusty_acceptance": [":acceptance_saved_firmware"], "//conditions:default": [":standard_saved_firmware"]})
    native.filegroup(name = "selected_saved_firmware", srcs = saved)
    extract_suffix = " $(@D)/cached $(locations :selected_saved_firmware) " + " ".join(["$(location " + label + ")" for label in selected])
    native.genrule(
        name = "cached_firmware",
        srcs = [":selected_saved_firmware"] + selected + scripts,
        outs = ["cached/" + name for name in names + ["build.prototxt"]],
        cmd = select({"//build/platforms:trusty_acceptance": command + "extract acceptance" + extract_suffix, "//conditions:default": command + "extract standard" + extract_suffix}),
    )
    native.sh_binary(
        name = "refresh_firmware",
        tags = ["manual"],
        srcs = ["test.sh"],
        data = scripts + [":built_firmware_bundle"] + selected,
        args = ["$(location bundle.py)", "$(location //tools/firmware:archive.py)", "refresh"] +
            select({"//build/platforms:trusty_acceptance": ["acceptance", "boot/efi/acceptance_firmware.bin"], "//conditions:default": ["standard", "boot/efi/standard_firmware.bin"]}) +
            ["$(location :built_firmware_bundle)"] + ["$(location " + label + ")" for label in selected],
    )
