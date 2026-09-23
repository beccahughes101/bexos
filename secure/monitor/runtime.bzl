"""Real saved Trusty images running under the in-tree SVM runtime."""
load("//build/platforms:architecture.bzl", "architecture_files")
load("@rules_rust//rust:defs.bzl", "rust_binary")
load("//boot/multiboot:boot_image.bzl", "multiboot_image")
load("//build/rules:optimized_artifact.bzl", "optimized_artifact")

def trusty_domain(name, variant, acceptance = False, normal_world = False, boot_ipc_probe = False, boot_approval = False, boot_release = False, require_efi = False, secure_product = False, external_payload = False, rollback_fixture = False, checkpoint_probe = False, normal_checkpoint_probe = False, selection_probe = False, monitor_candidate = None, monitor_candidate_image = False, firmware_recovery_probe = False, trusty_recovery_candidate = None, repeated_transfer = False, monitor_entry_failure = None, monitor_rollback_probe = False, nucleus_probe = False, resident_nucleus = False):
    firmware = "//third_party/trusty:cached_" + variant + "/"
    if secure_product or rollback_fixture:
        firmware = "//third_party/trusty:product_x86/"
    resident_nucleus = resident_nucleus or nucleus_probe
    if resident_nucleus and not (nucleus_probe or secure_product):
        fail("resident nucleus requires a product or explicit diagnostic")
    monitor_transfer = monitor_candidate != None or monitor_candidate_image
    if monitor_entry_failure and (not monitor_candidate_image or monitor_entry_failure not in ["fault", "hang"]):
        fail("entry failure is restricted to a diagnostic monitor candidate")
    if monitor_transfer:
        if secure_product or require_efi or boot_approval or acceptance or selection_probe:
            fail("monitor transfer proof must remain an isolated diagnostic")
        boot_ipc_probe = True
    if firmware_recovery_probe:
        if not monitor_transfer or normal_world:
            fail("firmware recovery probe requires an isolated monitor transfer")
    if selection_probe:
        if secure_product or normal_world or require_efi or boot_approval or acceptance:
            fail("selection journal probe must remain an isolated diagnostic")
        boot_ipc_probe = True
    if trusty_recovery_candidate:
        if monitor_transfer or secure_product or normal_world or require_efi or boot_approval or acceptance or selection_probe:
            fail("Trusty recovery probe requires an isolated cold-boot target")
        boot_ipc_probe = True
    if nucleus_probe:
        if monitor_transfer or secure_product or require_efi or boot_approval:
            fail("nucleus proof must be a separate execution owner")
        boot_ipc_probe = True
    normal = {}
    verified = {"RECOVERY_ROOT": ":boot_root"} if firmware_recovery_probe or trusty_recovery_candidate or nucleus_probe else {}
    if normal_checkpoint_probe:
        if secure_product or require_efi or not normal_world:
            fail("normal checkpoint trigger is restricted to a development fixture")
    if checkpoint_probe:
        if secure_product or normal_world or require_efi or boot_approval:
            fail("checkpoint diagnostic must remain a standalone monitor probe")
        boot_ipc_probe = True
    if external_payload and not boot_approval and not secure_product:
        fail("external payloads require authenticated boot approval")
    if rollback_fixture and (secure_product or normal_world or not boot_approval or not require_efi):
        fail("rollback provisioning is restricted to an authenticated, non-product fixture")
    if secure_product:
        normal_world = True
        boot_release = True
        require_efi = True
    if boot_release:
        boot_approval = True
    if boot_approval:
        boot_ipc_probe = True
        verified = {"VERIFIED_" + key: ":product_payload" + suffix for key, suffix in {
            "KERNEL": "_kernel", "BOOTFS": "_bootfs", "METADATA": ".vbmeta", "ROOT": ".avbpubkey",
        }.items()}
        if external_payload:
            verified = {"VERIFIED_ROOT": ":boot_root"}
    if normal_world:
        payloads = {"KERNEL": "//kernel:kernel", "BOOTFS": "//device/virtual/qemu/nongui:bootfs.emulated.img", "EVIDENCE": "//device/virtual/qemu/nongui:boot_evidence.emulated.bin"}
        if secure_product:
            payloads = {}
        for key, label in payloads.items():
            target = name + "_" + key.lower()
            architecture_files(name = target, srcs = [label], architecture = "x86_64")
            normal["NORMAL_" + key] = ":" + target
    if normal_world:
        architecture_files(name = name + "_disk", srcs = ["//device/virtual/qemu/nongui:qemu_nvme_gpt.img" if secure_product else "//device/virtual/qemu/nongui:qemu_nvme_development_gpt.img"], architecture = "x86_64")
    policy = {"MONITOR_POLICY": ":monitor_policy_baseline"} if resident_nucleus else {}
    if nucleus_probe:
        policy |= {"POLICY_" + kind.upper(): ":monitor_policy_" + kind + ".fw" for kind in ["fault", "hang", "candidate", "successor"]}
    # The diagnostic nucleus embeds the complete guest images inside its fixed
    # resident region. Keep that closure optimized, including in host test gates.
    rust_binary(
        name = name + "_binary" if nucleus_probe else name,
        srcs = ["runtime/main.rs", "runtime/platform.rs", "runtime/platform_state.rs", "runtime/secure_state.rs", "runtime/memory.rs", "runtime/devices.rs", "runtime/console.rs", "runtime/rtc.rs", "runtime/root.rs", "runtime/pci.rs", "runtime/pci_state.rs", "runtime/transport.rs", "runtime/transport_state.rs"] + (["runtime/normal.rs", "runtime/normal_state.rs", "runtime/domain_state.rs", "runtime/handoff.rs", "runtime/schedule.rs"] if normal_world else []) + (["runtime/approval.rs"] if boot_approval else []) + (["runtime/payload.rs"] if external_payload else []) + (["runtime/rollback_fixture.rs"] if rollback_fixture else []) + (["runtime/replacement.rs", "runtime/firmware_generations.rs"] if secure_product else []) + (["runtime/checkpoint_probe.rs"] if checkpoint_probe else []) + (["runtime/selection_probe.rs"] if selection_probe else []) + (["runtime/monitor_transfer.rs"] if monitor_transfer else []) + (["runtime/firmware_recovery.rs"] if firmware_recovery_probe else []) + (["runtime/secure_boot.rs", "runtime/trusty_recovery.rs"] if trusty_recovery_candidate else []) + (["runtime/recovery_faults.rs", "runtime/recovery_staging_faults.rs"] if firmware_recovery_probe or trusty_recovery_candidate or nucleus_probe else []) + (["runtime/nucleus.rs", "runtime/policy_guard.rs"] if resident_nucleus else []) + (["runtime/nucleus_probe.rs"] if nucleus_probe else []) + (["runtime/secure_boot.rs", "runtime/product_recovery.rs", "runtime/firmware_activation.rs", "runtime/trusty_owner.rs"] if resident_nucleus and secure_product else []),
        crate_features = (["normal_world"] if normal_world else []) + (["boot_ipc_probe"] if boot_ipc_probe else []) + (["boot_approval"] if boot_approval else []) + (["boot_release"] if boot_release else []) + (["require_efi"] if require_efi else []) + (["secure_product"] if secure_product else []) + (["external_payload"] if external_payload else []) + (["rollback_fixture"] if rollback_fixture else []) + (["checkpoint_probe"] if checkpoint_probe else []) + (["normal_checkpoint_probe"] if normal_checkpoint_probe else []) + (["selection_probe"] if selection_probe else []) + (["monitor_transfer"] if monitor_transfer else []) + (["monitor_candidate"] if monitor_candidate_image else []) + (["monitor_successor"] if monitor_candidate_image and monitor_candidate else []) + (["monitor_entry_" + monitor_entry_failure] if monitor_entry_failure else []) + (["firmware_recovery_probe"] if firmware_recovery_probe else []) + (["trusty_recovery_probe"] if trusty_recovery_candidate else []) + (["resident_nucleus"] if resident_nucleus else []) + (["nucleus_probe"] if nucleus_probe else []),
        # Probe source is conditional so normal products have no diagnostic
        # checkpoint trigger or success marker.
        crate_root = "runtime/main.rs",
        crate_name = name,
        edition = "2024",
        platform = "//build/platforms:kernel_x86_64",
        linker_script = ":nucleus_layout.ld" if resident_nucleus else ":runtime_layout.ld",
        rustc_flags = ["-C", "panic=abort", "-C", "relocation-model=static", "-C", "no-redzone=yes"],
        compile_data = [firmware + "lk.elf"] + policy.values() + normal.values() + verified.values() + ([monitor_candidate] if monitor_candidate else []) + ([trusty_recovery_candidate] if trusty_recovery_candidate else []),
        rustc_env = {"TRUSTY_ELF": "$(execpath " + firmware + "lk.elf)"} | {key: "$(execpath " + value + ")" for key, value in (normal | verified | policy).items()} | ({"MONITOR_CANDIDATE": "$(execpath " + monitor_candidate + ")"} if monitor_candidate else {}) | ({"TRUSTY_CANDIDATE": "$(execpath " + trusty_recovery_candidate + ")"} if trusty_recovery_candidate else {}),
        deps = [":monitor", "//lib/secure_monitor_abi", "@crates__sha2-0.11.0//:sha2"] + (["//lib/trusty_boot"] if boot_ipc_probe else []) + (["//lib/boot"] if normal_world else []) + (["//lib/secure_firmware"] if secure_product or selection_probe or firmware_recovery_probe or trusty_recovery_candidate or nucleus_probe else []),
    )
    if nucleus_probe:
        optimized_artifact(name = name, src = ":" + name + "_binary")
    if monitor_candidate_image:
        return
    multiboot_image(name = name + "_boot", kernel = ":" + name, trusty = True)
    native.sh_test(
        name = name + "_boot_test",
        srcs = ["probe/test.sh"],
        args = [
            "$(location //third_party/trusty:run_x86.py)",
            "$(location :" + name + "_boot)",
            "$(location " + firmware + "rpmb_dev)",
            "$(location " + firmware + "RPMB_DATA)",
            "--timeout", "600" if normal_world or firmware_recovery_probe or trusty_recovery_candidate else "300" if acceptance or selection_probe else "60",
        ] + (["--marker", "'monitor-runtime: authenticated EFI entry required; refusing execution'"] if require_efi else ["--trusty-recovery" if trusty_recovery_candidate else "--firmware-recovery" if firmware_recovery_probe else "--boot-selection" if selection_probe else "--acceptance" if acceptance else "--smoke"]) + (["--memory-mib", "2048", "--disk", "$(location :" + name + "_disk)", "--marker", "'kernel: cpu3 boot path ready'", "--marker", "'kernel: Q35 HPET IOAPIC interrupt verified'", "--marker", "'kernel: cpu3 scheduler idle ready'", "--marker", "'userspace: entering ring3 appd'", "--marker", "'debugd: QEMU socket transport ready'", "--marker", "'appd: guest persistence and disk-only application verified'", "--marker", "'wasi-fixture: streams clocks random environment ok'"] if normal_world and not require_efi else []) + (["--marker", "'monitor-runtime: boot RPMB owner release verified'" if boot_release else "'monitor-runtime: authenticated payload and Trusty rollback approval verified'" if boot_approval else "'monitor-runtime: Trusty AVB IPC verified'"] if boot_ipc_probe and not require_efi else []) + (["--marker", "'monitor-runtime: real Trusty IPC continued after CPU and platform restore'", "--marker", "'monitor-runtime: pending Trusty request retained across transport restore'", "--marker", "'monitor-runtime: secure owner restored after atomic transport rejection'"] if checkpoint_probe else []) + (["--marker", "'monitor-runtime: four BexOS CPU records validated under permanent execution owner'", "--marker", "'monitor-runtime: both domain records validated after atomic handoff rejection'"] if normal_checkpoint_probe else []) + (["--memory-mib", "3072", "--marker", "'monitor-runtime: real Trusty IPC continued after distinct monitor entry and old-image reuse'"] if monitor_transfer and not monitor_rollback_probe else []) + (["--memory-mib", "3072", "--marker", "'monitor-runtime: resident recovery resumed old monitor and retained in-flight secure client'"] if monitor_rollback_probe else []) + (["--marker", "'monitor-runtime: repeated distinct monitor replacement reused both image banks'"] if repeated_transfer else []) + (["--memory-mib", "3072", "--nucleus"] if nucleus_probe else []),
        data = (["//third_party/trusty:firmware_recovery_test.py"] if firmware_recovery_probe or trusty_recovery_candidate else []) + ["//third_party/trusty:run_x86.py", ":" + name + "_boot", firmware + "rpmb_dev", firmware + "RPMB_DATA"] + ([":" + name + "_disk", "//third_party/trusty:x86_devices.py"] if normal_world else []),
        tags = ["requires-qemu", "exclusive", "local", "no-sandbox"],
        timeout = "eternal" if acceptance and normal_world else "long" if acceptance or normal_world or selection_probe or firmware_recovery_probe or trusty_recovery_candidate else "moderate",
    )
