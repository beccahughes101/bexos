load("//build/platforms:architecture.bzl", "guest_select")
load("//testing/e2e/qemu:qemu_e2e.bzl", "qemu_e2e_test", "qemu_development_e2e_test")
load("//build/rules:app_archive.bzl", "app_archive")

def rpmb_service_update_test(name, boot_data, package, generation, archive, manifest, timeout_manifest, executable, tier):
    faults = []
    for mode in ["reject", "incompatible", "failure", "timeout"]:
        target = name + "_" + mode
        app_archive(
            name = target,
            manifest = timeout_manifest if mode == "timeout" else manifest,
            entries = {executable: ":candidate_" + mode},
            compression = "zstd",
        )
        faults.append(":" + target)
    service_update_test(
        tier = tier,
        name = name,
        boot_data = boot_data,
        package = package,
        generation = generation,
        archive = archive,
        flags = ["--update-engine-faults"] + ["$(location " + f + ")" for f in faults],
        data = faults,
    )

def _stored_archive_id(package, generation, suffix = None):
    base = package + "-" + str(generation)
    if suffix:
        return base + "-" + suffix
    return base

def update_e2e_test(
        name,
        boot_data,
        scenario_args,
        tier,
        data = [],
        timeout = "eternal",
        x86_scenario_args = None,
        x86_data = None,
        development = False,
        architectures = ["aarch64", "x86_64"],
        secure_firmware = None,
        trusty_variant = "standard"):
    test_rule = qemu_development_e2e_test if development else qemu_e2e_test
    architecture_args = {} if development else {
        "architectures": architectures,
        "trusty_variant": trusty_variant,
    }
    if secure_firmware != None:
        architecture_args["secure_firmware"] = secure_firmware
    test_rule(
        tier = tier,
        name = name,
        src = "debugd_updated_e2e_test.rs",
        extra_srcs = ["firmware_staging.rs", "firmware_activation.rs"],
        crate_name = "update_e2e_test",
        boot_data = boot_data,
        extra_data = data,
        x86_extra_data = x86_data,
        extra_args = scenario_args,
        x86_extra_args = x86_scenario_args,
        deps = [
            "//idl:tee_manager_fidl_rust",
            "//idl:update_manager_fidl_rust",
            "//host/debug_client",
            "//lib/debug_wire",
            "//lib/crypto",
            "//lib/secure_firmware",
            "//lib/trace",
            "//lib/update:update_host",
            "//testing/e2e",
            "//tools/qemu:qemu_test",
        ],
        env = {"BEXOS_QEMU_TIMEOUT_SECONDS": "600"},
        timeout = timeout,
        **architecture_args
    )

def service_update_test(
        name,
        boot_data,
        package,
        generation,
        archive,
        tier,
        flags = [],
        data = [],
        prerequisites = [],
        prelude_app = None,
        prelude_tee = False,
        x86_package = None,
        x86_archive = None,
        development = False):
    replacements = {}
    if x86_package:
        replacements[package] = x86_package
    if x86_archive:
        native.alias(name = name + "_selected_archive", actual = guest_select(archive, x86_archive))
        archive = ":" + name + "_selected_archive"
    prerequisites = [dict(prerequisite) for prerequisite in prerequisites]
    for index, prerequisite in enumerate(prerequisites):
        if "x86_package" in prerequisite:
            replacements[prerequisite["package"]] = prerequisite["x86_package"]
        if "x86_archive" in prerequisite:
            selected = name + "_prerequisite_" + str(index)
            native.alias(name = selected, actual = guest_select(prerequisite["archive"], prerequisite["x86_archive"]))
            prerequisite["archive"] = ":" + selected
    prerequisite_args = []
    prerequisite_data = []
    update_entries = [{
        "archive": archive,
        "id": _stored_archive_id(package, generation),
    }]
    for prerequisite in prerequisites:
        prerequisite_args += [
            "--prerequisite",
            prerequisite["package"],
            str(prerequisite["generation"]),
            "$(rootpath " + prerequisite["archive"] + ")",
        ]
        prerequisite_data.append(prerequisite["archive"])
        update_entries.append({
            "archive": prerequisite["archive"],
            "id": _stored_archive_id(prerequisite["package"], prerequisite["generation"]),
        })
    prelude_args = []
    prelude_data = []
    if prelude_tee:
        prelude_args.append("--prelude-tee")
    if prelude_app:
        prelude_args += [
            "--prelude-app",
            "$(rootpath " + prelude_app + ")",
        ]
        prelude_data.append(prelude_app)
    stored_flags = []
    extra_data = list(data)
    skip_fault_locations = 0
    for flag in flags:
        if skip_fault_locations:
            skip_fault_locations -= 1
            continue
        if flag == "--update-engine-faults":
            fault_labels = data[:4]
            fault_ids = [
                _stored_archive_id(package, generation, "rejection"),
                _stored_archive_id(package, generation, "incompatible"),
                _stored_archive_id(package, generation, "failure"),
                _stored_archive_id(package, generation, "timeout"),
            ]
            for index in range(4):
                update_entries.append({
                    "archive": fault_labels[index],
                    "id": fault_ids[index],
                })
            stored_flags.append("--update-engine-faults")
            stored_flags.extend(fault_ids)
            skip_fault_locations = 4
        else:
            stored_flags.append(flag)
    disk = ":" + name + "_nvme_gpt"
    command = ("cp $(location " + boot_data["disk"] + ") $@ && chmod u+w $@ && " +
              "$(location //tools/image:bexfs_image) --inspect --image $@ --partition STORAGE --label STORAGE " +
              "--key-file $(location " + boot_data["key"] + ") " +
              "".join([
                  "--entry updates/%s.bex=$(location %s) " % (entry["id"], entry["archive"])
                  for entry in update_entries
              ]))
    native.genrule(
        name = name + "_nvme_gpt",
        srcs = [
            boot_data["disk"],
            boot_data["key"],
        ] + [entry["archive"] for entry in update_entries],
        outs = [name + "_nvme.img"],
        cmd = guest_select(command, _replace_all(command, replacements)),
        tools = ["//tools/image:bexfs_image"],
    )
    test_boot_data = dict(boot_data)
    test_boot_data["disk"] = disk
    if development:
        stored_flags.append("--development")
    scenario_args = ["service", package, str(generation), "$(rootpath " + archive + ")"] + prelude_args + prerequisite_args + stored_flags
    update_e2e_test(
        tier = tier,
        name = name,
        boot_data = test_boot_data,
        scenario_args = scenario_args,
        x86_scenario_args = [_replace_all(arg, replacements) for arg in scenario_args],
        data = [archive] + prelude_data + prerequisite_data + extra_data,
        development = development,
    )

def _replace_all(value, replacements):
    for old, new in replacements.items():
        value = value.replace(old, new)
    return value


def development_service_tests(name, boot_data, services, presubmit_packages = []):
    tests = []
    for entry in services:
        target = name + "_" + entry["package"].split(".")[-1]
        service_update_test(
            tier = "presubmit" if entry["package"] in presubmit_packages else "extended",
            name = target,
            boot_data = boot_data,
            development = True,
            package = entry["package"],
            generation = entry["generation"],
            archive = entry["archive"],
            flags = entry.get("flags", []),
        )
        tests.append(":" + target + "_x86_64")
    native.test_suite(name = name + "_x86_64", tests = tests)
