load("//testing/e2e/qemu:qemu_e2e.bzl", "qemu_e2e_test", "qemu_development_e2e_test")

def network_transplant_test(name, boot_data, development):
    archives = ["//drivers/d1/nic/virtio/net:replacement_archive", "//services/netstack:replacement_archive", "//services/keychaind:replacement_archive"]
    native.genrule(
        name = name + "_disk",
        srcs = [boot_data["disk"], boot_data["key"]] + archives,
        outs = [name + ".img"],
        tools = ["//tools/image:bexfs_image"],
        cmd = "cp $(location " + boot_data["disk"] + ") $@ && chmod u+w $@ && $(location //tools/image:bexfs_image) --inspect --image $@ --partition STORAGE --label STORAGE --key-file $(location " + boot_data["key"] + ") " + " ".join(["--entry updates/network-%d.bex=$(location %s)" % (71 + i, archive) for i, archive in enumerate(archives)]),
    )
    boot = dict(boot_data)
    boot["disk"] = ":" + name + "_disk"
    rule = qemu_development_e2e_test if development else qemu_e2e_test
    rule(
        tier = "extended",
        name = name, src = "elf_e2e_test.rs", extra_srcs = ["echo.rs", "transplant.rs"], crate_name = "network_transplant_test",
        boot_data = boot,
        deps = ["//testing/e2e", "//tools/qemu:qemu_test", "//host/debug_client"],
        extra_data = [":library_archive", ":probe_archive"],
        extra_args = ["$(rootpath :library_archive)", "$(rootpath :probe_archive)", "transplant"],
    )
