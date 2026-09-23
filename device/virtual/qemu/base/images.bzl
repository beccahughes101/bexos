"""Product-local boot, AVB, and writable storage images."""
load("//build/platforms:architecture.bzl", "guest_select")
load("//build/rules:app_archive.bzl", "app_archive")
load("//lib/flatland_text:BUILD.fonts.bzl", "FONT_LICENSES", "SYSTEM_FONT_ENTRIES")

QEMU_STORAGE_SIZE_BYTES = 335544320

QEMU_STORAGE_PREINSTALLS = [
    {"archive": "//services/pkgd:pkgd_archive", "path": "pkg/bexos.service.pkgd.bex"},
    {"archive": "//services/pkgd:replacement_archive", "path": "updates/bexos.service.pkgd.replacement.bex"},
    {"archive": "//apps/brush_shell", "path": "pkg/bexos.app.brush_shell.bex"},
    {"archive": "//apps/brush_shell:replacement_archive", "path": "updates/bexos.app.brush_shell.replacement.bex"},
    {"archive": "//testing/wasm:file_service_archive", "path": "pkg/bexos.test.wasm.file_service.bex"},
    {"archive": "//testing/wasm:file_client_archive", "path": "pkg/bexos.test.wasm.file_client.bex"},
    {"archive": "//testing/wasm:file_replacement_archive", "path": "updates/bexos.test.wasm.file_service.replacement.bex"},
    {"archive": "//testing/wasm:memory_archive", "path": "pkg/bexos.test.wasm.memory.bex"},
    {"archive": "//testing/wasm:network_denied_archive", "path": "pkg/bexos.test.wasm.network_denied.bex"},
    {"archive": "//testing/wasm:network_granted_archive", "path": "pkg/bexos.test.wasm.network_granted.bex"},
    {"archive": "//testing/wasm:children_archive", "path": "pkg/bexos.test.wasm.children.bex"},
    {"archive": "//testing/wasm:client_archive", "path": "pkg/bexos.test.wasm.client.bex"},
    {"archive": "//testing/wasm:runner_replacement_archive", "path": "updates/bexos.test.wasm.service.runner_replacement.bex"},
    {"archive": "//testing/wasm:replacement_archive", "path": "updates/bexos.test.wasm.service.replacement.bex"},
    {"archive": "//testing/wasm:rejected_restore_archive", "path": "updates/bexos.test.wasm.service.rejected.bex"},

    {"archive": "//testing/wasm:command_archive", "path": "pkg/bexos.test.wasm.command.bex"},
    {"archive": "//testing/wasm:trap_archive", "path": "pkg/bexos.test.wasm.trap.bex"},
    {"archive": "//testing/wasm:service_archive", "path": "pkg/bexos.test.wasm.service.bex"},

    {"archive": "//services/wasm_runner:wasm_runner_archive", "path": "pkg/bexos.platform.wasm_runner.bex"},
    {
        "archive": ":storage_verify_archive",
        "path": "pkg/bexos.platform.storage_verify.bex",
    },
    {
        "archive": "//lib/crypto:crypto_archive",
        "path": "pkg/bexos.lib.crypto.bex",
    },
    {
        "archive": "//lib/net:net_archive",
        "path": "pkg/bexos.lib.net.bex",
    },
    {
        "archive": "//lib/tee_driver_trusty:tee_driver_trusty_archive",
        "path": "pkg/bexos.lib.tee_driver.trusty.bex",
    },
    {
        "archive": "//lib/i18n/settings:locale_archive",
        "path": "pkg/bexos.locale.preferences.bex",
    },
    {
        "archive": "//services/localed:localed_archive",
        "path": "pkg/bexos.service.localed.bex",
    },
    {
        "archive": "//lib/ui/theme:theme_archive",
        "path": "pkg/bexos.ui.theme.bex",
    },
    {
        "archive": ":virtio_net_archive",
        "path": "pkg/bexos.driver.network.virtio_net.bex",
    },
    {
        "archive": ":qemu_netstackd_archive",
        "path": "pkg/bexos.service.netstackd.bex",
    },
    {
        "archive": ":qemu_timed_archive",
        "path": "pkg/bexos.service.timed.bex",
    },
    {
        "archive": "//services/prefsd:prefsd_archive",
        "path": "pkg/bexos.service.prefsd.bex",
    },
    {
        "archive": "//services/jobd:jobd_archive",
        "path": "pkg/bexos.service.jobd.bex",
    },
]

QEMU_STORAGE_PREINSTALLS_WITHOUT_BRUSH = [
    entry
    for entry in QEMU_STORAGE_PREINSTALLS
    if entry["archive"] not in [
        "//apps/brush_shell",
        "//apps/brush_shell:replacement_archive",
    ]
]

QEMU_STORAGE_PREINSTALLS_SHELL_FIXTURE = QEMU_STORAGE_PREINSTALLS_WITHOUT_BRUSH + [
    {
        "archive": "//testing/e2e/qemu/bexfs:shell_fixture_archive",
        "path": "pkg/com.example.shell_fixture.bex",
    },
]

def qemu_images(graphics = False, input_fixture = False):
    graphics_packages = [('drivers/d1/input/virtio', 'bexos.driver.input.virtio', 'input', 'input_driver'), ('drivers/d1/display/virtio/gpu', 'bexos.driver.display.virtio_gpu', 'gpu', 'gpu_driver'), ('services/splashd', 'bexos.service.splashd', 'splashd', 'splashd_elf'), ('services/fontd', 'bexos.service.fontd', 'fontd', 'fontd_elf'), ('services/scened', 'bexos.service.scened', 'scened', 'scened_elf')] if graphics else []
    if input_fixture:
        graphics_packages.append(('testing/e2e/qemu/graphics/input_fixture', 'bexos.testing.input_fixture', 'input_fixture', 'fixture_elf'))
    graphics_inputs = ["//%s:%s" % (path, target) for path, pkg, name, target in graphics_packages] + ["//%s:package_manifest" % path for path, pkg, name, target in graphics_packages]
    graphics_entries = "".join(["--elf /boot/pkg/%s/bin/%s=$(location //%s:%s) --entry /boot/pkg/%s/package.bexmanifest=$(location //%s:package_manifest) " % (pkg, name, path, target, pkg, path) for path, pkg, name, target in graphics_packages])
    graphics_inputs += SYSTEM_FONT_ENTRIES.values() + FONT_LICENSES.values()
    graphics_entries += "".join(["--entry /system/%s=$(location %s) " % (path, source) for path, source in SYSTEM_FONT_ENTRIES.items()])
    graphics_entries += "".join(["--entry /system/data/fonts/%s=$(location %s) " % (path, source) for path, source in FONT_LICENSES.items()])
    if graphics:
        graphics_inputs += ["//services/scened:desktop_config"]
        graphics_inputs += ["//third_party/mesa:notices", "@nanoprintf//:LICENSE"]
        graphics_entries += "--entry /boot/pkg/bexos.service.scened/licenses/mesa/NOTICES.txt=$(location //third_party/mesa:notices) "
        graphics_entries += "--entry /boot/pkg/bexos.service.scened/licenses/nanoprintf/LICENSE=$(location @nanoprintf//:LICENSE) "
        graphics_entries += "".join(["--entry /boot/pkg/bexos.service.scened/%s=$(location %s) " % (path, source) for path, source in FONT_LICENSES.items()])
        graphics_entries += "--entry /boot/pkg/bexos.service.scened/config/desktop.pb=$(location //services/scened:desktop_config) "
    storage_preinstalls = QEMU_STORAGE_PREINSTALLS + [{"archive": "//%s:replacement_archive" % path, "path": "updates/%s.replacement.bex" % pkg} for path, pkg, name, target in graphics_packages]
    if graphics:
        storage_preinstalls += [
            {"archive": "//drivers/d1/input/virtio:input_archive", "path": "pkg/bexos.driver.input.virtio.bex"},
            {"archive": "//apps/dioxus_shared:dioxus_shared", "path": "pkg/com.bexos.lib.dioxus.bex"},
            {"archive": "//apps/dioxus_demo:dioxus_demo", "path": "pkg/bexos.app.dioxus_demo.bex"},
            {"archive": "//apps/sysui:sysui", "path": "pkg/bexos.app.sysui.bex"},
            {"archive": "//apps/userui:userui", "path": "pkg/bexos.app.userui.bex"},
            {"archive": "//apps/sysui:replacement_archive", "path": "updates/bexos.app.sysui.replacement.bex"},
            {"archive": "//apps/userui:replacement_archive", "path": "updates/bexos.app.userui.replacement.bex"},
            {"archive": "//apps/dioxus_demo:replacement_archive", "path": "updates/bexos.app.dioxus_demo.replacement.bex"},
        ]
    if input_fixture:
        storage_preinstalls.append({"archive": "//drivers/d1/bus/generic/pci:replacement_archive", "path": "updates/bexos.driver.pci_root.replacement.bex"})
        storage_preinstalls += [
            {"archive": "//testing/e2e/qemu/graphics:scened_reject_archive", "path": "updates/bexos.service.scened.rejected.bex"},
            {"archive": "//testing/e2e/qemu/graphics:gpu_reject_archive", "path": "updates/bexos.driver.display.virtio_gpu.rejected.bex"},
        ]
    native.genrule(
        name = "bootfs_image",
        srcs = graphics_inputs + [
            "//drivers/d1/serial/virtio/console:package_manifest",
            "//drivers/d1/serial/virtio/console:virtio_console_driver",
            ":bootfs_manifest_bin", ":platform_config_bin", ":product_assembly_index", "//device/virtual/qemu/base:qemu_bexfs_test.key",
            "//services/appd:appd_elf",
            "//services/appd:package_manifest",
            "//services/wasm_runner:wasm_runner_elf",
            "//testing/wasm:boot_wasm", "//testing/wasm:boot_manifest",
            "//drivers/d1/bus/generic/pci:package_manifest",
            "//drivers/d1/storage/nvmexpress/nvme:package_manifest", "//drivers/d1/storage/bexos/bexfs:package_manifest",
            "//drivers/d1/storage/bexos/bexfs:user_package_manifest",
            "//drivers/d1/storage/bexos/archivefs:package_manifest",
            "//drivers/d1/storage/bexos/memfs:package_manifest",
            "//drivers/d1/storage/bexos/diskimage:package_manifest",
            "//services/vfsd:package_manifest",
            "//services/teed:package_manifest",
            "//services/debugd:package_manifest",
            "//services/traced:package_manifest",
            "//services/updated:package_manifest",
            "//services/trustd:package_manifest",
            "//services/powerd:package_manifest",
            "//services/usersd:package_manifest",
            "//services/keychaind:package_manifest",
            "//lib/crypto:package_manifest",
            "//lib/crypto:crypto_shared",
            "//lib/net:package_manifest",
            "//lib/net:net_shared",
            "//lib/tee_driver_trusty:package_manifest",
            "//lib/tee_driver_trusty:tee_driver_trusty_shared",
            "//lib/tee_driver_trusty:tee_driver_trusty_archive",
            "//lib/tee_driver_software:tee_driver_software_archive",
            "//ecosystem/bexos:public_bundle",
            "//ecosystem/bexos:tls_roots_redb",
            "//ecosystem/bexos:app_signing_roots_redb",
            "//drivers/d1/bus/generic/pci:pci_root_bus",
            "//drivers/d1/storage/nvmexpress/nvme:nvme_driver", "//drivers/d1/storage/bexos/bexfs:bexfs_driver",
            "//drivers/d1/storage/bexos/bexfs:user_bexfs_driver",
            "//drivers/d1/storage/bexos/archivefs:archivefs_driver",
            "//drivers/d1/storage/bexos/memfs:memfs_driver",
            "//drivers/d1/storage/bexos/diskimage:diskimage_driver",
            "//services/vfsd:vfsd_elf",
            "//services/teed:teed_elf",
            "//services/debugd:debugd_elf",
            "//services/traced:traced_elf",
            "//services/updated:updated_elf",
            "//services/trustd:trustd_elf",
            "//services/powerd:powerd_elf",
            "//services/usersd:usersd_elf",
            "//services/keychaind:keychaind_elf",
        ] + guest_select(["//drivers/d1/serial/arm/pl011:package_manifest", "//drivers/d1/rtc/arm/pl031:package_manifest", "//drivers/d1/serial/arm/pl011:pl011", "//drivers/d1/rtc/arm/pl031:pl031"], ["//drivers/d1/rtc/pc/cmos:cmos", "//drivers/d1/rtc/pc/cmos:package_manifest"]),
        outs = ["bootfs.img"],
        cmd = "$(location //tools/image:assemble_bootfs) --manifest-validator $(location //tools/app_manifest:stamp) --out $@ " + graphics_entries + guest_select("--architecture aarch64 ", "--architecture x86_64 ") +
              "--entry /boot/pkg/bexos.driver.serial.virtio_console/package.bexmanifest=$(location //drivers/d1/serial/virtio/console:package_manifest) " +
              "--entry /boot/manifest/bootfs_manifest.bin=$(location :bootfs_manifest_bin) " +
              "--entry /boot/manifest/product.assembly=$(location :product_assembly_index) " +
              "--entry /boot/platform.pcfg=$(location :platform_config_bin) " +
              "--entry /boot/qemu-test.key=$(location //device/virtual/qemu/base:qemu_bexfs_test.key) " +
              "--entry /boot/pkg/bexos.platform.appd/package.bexmanifest=$(location //services/appd:package_manifest) " +
              "--entry /boot/pkg/bexos.driver.pci_root/package.bexmanifest=$(location //drivers/d1/bus/generic/pci:package_manifest) " +
              guest_select("--entry /boot/pkg/bexos.driver.uart.pl011/package.bexmanifest=$(location //drivers/d1/serial/arm/pl011:package_manifest) ", "") +
              guest_select("--entry /boot/pkg/bexos.driver.rtc.pl031/package.bexmanifest=$(location //drivers/d1/rtc/arm/pl031:package_manifest) ", "--entry /boot/pkg/bexos.driver.rtc.cmos/package.bexmanifest=$(location //drivers/d1/rtc/pc/cmos:package_manifest) ") +
              "--entry /boot/pkg/bexos.driver.storage.nvme/package.bexmanifest=$(location //drivers/d1/storage/nvmexpress/nvme:package_manifest) " +
              "--entry /boot/pkg/bexos.driver.storage.bexfs/package.bexmanifest=$(location //drivers/d1/storage/bexos/bexfs:package_manifest) " +
              "--entry /boot/pkg/bexos.driver.storage.user_bexfs/package.bexmanifest=$(location //drivers/d1/storage/bexos/bexfs:user_package_manifest) " +
              "--entry /boot/pkg/bexos.driver.storage.archivefs/package.bexmanifest=$(location //drivers/d1/storage/bexos/archivefs:package_manifest) " +
              "--entry /boot/pkg/bexos.driver.storage.memfs/package.bexmanifest=$(location //drivers/d1/storage/bexos/memfs:package_manifest) " +
              "--entry /boot/pkg/bexos.driver.storage.diskimage/package.bexmanifest=$(location //drivers/d1/storage/bexos/diskimage:package_manifest) " +
              "--entry /boot/pkg/bexos.service.vfsd/package.bexmanifest=$(location //services/vfsd:package_manifest) " +
              "--entry /boot/pkg/bexos.service.teed/package.bexmanifest=$(location //services/teed:package_manifest) " +
              "--entry /boot/pkg/bexos.driver.debugd/package.bexmanifest=$(location //services/debugd:package_manifest) " +
              "--entry /boot/pkg/bexos.service.traced/package.bexmanifest=$(location //services/traced:package_manifest) " +
              "--entry /boot/pkg/bexos.service.updated/package.bexmanifest=$(location //services/updated:package_manifest) " +
              "--entry /boot/pkg/bexos.service.trustd/package.bexmanifest=$(location //services/trustd:package_manifest) " +
              "--entry /boot/pkg/bexos.service.powerd/package.bexmanifest=$(location //services/powerd:package_manifest) " +
              "--entry /boot/pkg/bexos.service.usersd/package.bexmanifest=$(location //services/usersd:package_manifest) " +
              "--entry /boot/pkg/bexos.service.keychaind/package.bexmanifest=$(location //services/keychaind:package_manifest) " +
              "--entry /boot/pkg/bexos.lib.crypto/package.bexmanifest=$(location //lib/crypto:package_manifest) " +
              "--entry /boot/pkg/bexos.lib.crypto/lib/libbexos_crypto.so=$(location //lib/crypto:crypto_shared) " +
              "--entry /boot/pkg/bexos.lib.net/package.bexmanifest=$(location //lib/net:package_manifest) " +
              "--entry /boot/pkg/bexos.lib.net/lib/libbexos_net.so=$(location //lib/net:net_shared) " +
              "--entry /boot/pkg/bexos.lib.tee_driver.trusty/package.bexmanifest=$(location //lib/tee_driver_trusty:package_manifest) " +
              "--entry /boot/pkg/bexos.lib.tee_driver.trusty/lib/libbexos_tee_driver.so=$(location //lib/tee_driver_trusty:tee_driver_trusty_shared) " +
              "--entry /system/pkg/bexos.lib.tee_driver.trusty.bex=$(location //lib/tee_driver_trusty:tee_driver_trusty_archive) " +
              "--entry /system/pkg/bexos.lib.tee_driver.software.bex=$(location //lib/tee_driver_software:tee_driver_software_archive) " +
              "--entry /system/ecosystem/bexos.bundle=$(location //ecosystem/bexos:public_bundle) " +
              "--entry /system/certs/tls_roots.redb=$(location //ecosystem/bexos:tls_roots_redb) " +
              "--entry /system/certs/app_signing_roots.redb=$(location //ecosystem/bexos:app_signing_roots_redb) " +
              "--elf /boot/pkg/bexos.driver.serial.virtio_console/bin/virtio_console_driver=$(location //drivers/d1/serial/virtio/console:virtio_console_driver) " +
              "--elf /boot/pkg/bexos.platform.appd/bin/appd=$(location //services/appd:appd_elf) " +
              "--entry /boot/pkg/bexos.platform.wasm_runner/bin/wasm_runner=$(location //services/wasm_runner:wasm_runner_elf) " +
              "--entry /boot/pkg/bexos.test.wasm.boot/package.bexmanifest=$(location //testing/wasm:boot_manifest) " +
              "--entry /boot/pkg/bexos.test.wasm.boot/bin/boot.wasm=$(location //testing/wasm:boot_wasm) " +
              "--elf /boot/pkg/bexos.driver.pci_root/bin/pci_root_bus=$(location //drivers/d1/bus/generic/pci:pci_root_bus) " +
              guest_select("--elf /boot/pkg/bexos.driver.uart.pl011/bin/pl011=$(location //drivers/d1/serial/arm/pl011:pl011) ", "") +
              guest_select("--elf /boot/pkg/bexos.driver.rtc.pl031/bin/pl031=$(location //drivers/d1/rtc/arm/pl031:pl031) ", "--elf /boot/pkg/bexos.driver.rtc.cmos/bin/cmos=$(location //drivers/d1/rtc/pc/cmos:cmos) ") +
              "--elf /boot/pkg/bexos.driver.storage.nvme/bin/nvme_driver=$(location //drivers/d1/storage/nvmexpress/nvme:nvme_driver) " +
              "--elf /boot/pkg/bexos.driver.storage.bexfs/bin/bexfs_driver=$(location //drivers/d1/storage/bexos/bexfs:bexfs_driver) " +
              "--elf /boot/pkg/bexos.driver.storage.user_bexfs/bin/user_bexfs_driver=$(location //drivers/d1/storage/bexos/bexfs:user_bexfs_driver) " +
              "--elf /boot/pkg/bexos.driver.storage.archivefs/bin/archivefs_driver=$(location //drivers/d1/storage/bexos/archivefs:archivefs_driver) " +
              "--elf /boot/pkg/bexos.driver.storage.memfs/bin/memfs_driver=$(location //drivers/d1/storage/bexos/memfs:memfs_driver) " +
              "--elf /boot/pkg/bexos.driver.storage.diskimage/bin/diskimage_driver=$(location //drivers/d1/storage/bexos/diskimage:diskimage_driver) " +
              "--elf /boot/pkg/bexos.service.vfsd/bin/vfsd=$(location //services/vfsd:vfsd_elf) " +
              "--elf /boot/pkg/bexos.service.teed/bin/teed=$(location //services/teed:teed_elf) " +
              "--elf /boot/pkg/bexos.driver.debugd/bin/debugd=$(location //services/debugd:debugd_elf) " +
              "--elf /boot/pkg/bexos.service.traced/bin/traced=$(location //services/traced:traced_elf) " +
              "--elf /boot/pkg/bexos.service.updated/bin/updated=$(location //services/updated:updated_elf) " +
              "--elf /boot/pkg/bexos.service.trustd/bin/trustd=$(location //services/trustd:trustd_elf) " +
              "--elf /boot/pkg/bexos.service.powerd/bin/powerd=$(location //services/powerd:powerd_elf) " +
              "--elf /boot/pkg/bexos.service.usersd/bin/usersd=$(location //services/usersd:usersd_elf) " +
              "--elf /boot/pkg/bexos.service.keychaind/bin/keychaind=$(location //services/keychaind:keychaind_elf)",
        tools = ["//tools/image:assemble_bootfs", "//tools/app_manifest:stamp"],
    )

    native.genrule(
        name = "bootfs_image_emulated",
        srcs = graphics_inputs + [
            "//drivers/d1/serial/virtio/console:package_manifest",
            "//drivers/d1/serial/virtio/console:virtio_console_driver",
            ":bootfs_manifest_bin", ":platform_config_emulated_bin", ":product_emulated_assembly_index", "//device/virtual/qemu/base:qemu_bexfs_test.key",
            "//services/appd:appd_elf",
            "//services/appd:package_manifest",
            "//services/wasm_runner:wasm_runner_elf",
            "//testing/wasm:boot_wasm", "//testing/wasm:boot_manifest",
            "//drivers/d1/bus/generic/pci:package_manifest",
            "//drivers/d1/storage/nvmexpress/nvme:package_manifest", "//drivers/d1/storage/bexos/bexfs:package_manifest",
            "//drivers/d1/storage/bexos/bexfs:user_package_manifest",
            "//drivers/d1/storage/bexos/archivefs:package_manifest",
            "//drivers/d1/storage/bexos/memfs:package_manifest",
            "//drivers/d1/storage/bexos/diskimage:package_manifest",
            "//services/vfsd:package_manifest",
            "//services/teed:package_manifest_software",
            "//services/debugd:package_manifest",
            "//services/traced:package_manifest",
            "//services/updated:package_manifest",
            "//services/trustd:package_manifest",
            "//services/powerd:package_manifest",
            "//services/usersd:package_manifest",
            "//services/keychaind:package_manifest",
            "//lib/crypto:package_manifest",
            "//lib/crypto:crypto_shared",
            "//lib/net:package_manifest",
            "//lib/net:net_shared",
            "//lib/tee_driver_software:package_manifest",
            "//lib/tee_driver_software:tee_driver_software_shared",
            "//lib/tee_driver_trusty:tee_driver_trusty_archive",
            "//lib/tee_driver_software:tee_driver_software_archive",
            "//ecosystem/bexos:public_bundle",
            "//ecosystem/bexos:tls_roots_redb",
            "//ecosystem/bexos:app_signing_roots_redb",
            "//drivers/d1/bus/generic/pci:pci_root_bus",
            "//drivers/d1/storage/nvmexpress/nvme:nvme_driver", "//drivers/d1/storage/bexos/bexfs:bexfs_driver",
            "//drivers/d1/storage/bexos/bexfs:user_bexfs_driver",
            "//drivers/d1/storage/bexos/archivefs:archivefs_driver",
            "//drivers/d1/storage/bexos/memfs:memfs_driver",
            "//drivers/d1/storage/bexos/diskimage:diskimage_driver",
            "//services/vfsd:vfsd_elf",
            "//services/teed:teed_elf",
            "//services/debugd:debugd_elf",
            "//services/traced:traced_elf",
            "//services/updated:updated_elf",
            "//services/trustd:trustd_elf",
            "//services/powerd:powerd_elf",
            "//services/usersd:usersd_elf",
            "//services/keychaind:keychaind_elf",
        ] + guest_select(["//drivers/d1/serial/arm/pl011:package_manifest", "//drivers/d1/rtc/arm/pl031:package_manifest", "//drivers/d1/serial/arm/pl011:pl011", "//drivers/d1/rtc/arm/pl031:pl031"], ["//drivers/d1/rtc/pc/cmos:cmos", "//drivers/d1/rtc/pc/cmos:package_manifest"]),
        outs = ["bootfs.emulated.img"],
        cmd = "$(location //tools/image:assemble_bootfs) --manifest-validator $(location //tools/app_manifest:stamp) --out $@ " + graphics_entries + guest_select("--architecture aarch64 ", "--architecture x86_64 ") +
              "--entry /boot/pkg/bexos.driver.serial.virtio_console/package.bexmanifest=$(location //drivers/d1/serial/virtio/console:package_manifest) --elf /boot/pkg/bexos.driver.serial.virtio_console/bin/virtio_console_driver=$(location //drivers/d1/serial/virtio/console:virtio_console_driver) " +
              "--entry /boot/manifest/bootfs_manifest.bin=$(location :bootfs_manifest_bin) " +
              "--entry /boot/manifest/product.assembly=$(location :product_emulated_assembly_index) " +
              "--entry /boot/platform.pcfg=$(location :platform_config_emulated_bin) " +
              "--entry /boot/qemu-test.key=$(location //device/virtual/qemu/base:qemu_bexfs_test.key) " +
              "--entry /boot/pkg/bexos.platform.appd/package.bexmanifest=$(location //services/appd:package_manifest) " +
              "--entry /boot/pkg/bexos.driver.pci_root/package.bexmanifest=$(location //drivers/d1/bus/generic/pci:package_manifest) " +
              guest_select("--entry /boot/pkg/bexos.driver.uart.pl011/package.bexmanifest=$(location //drivers/d1/serial/arm/pl011:package_manifest) ", "") +
              guest_select("--entry /boot/pkg/bexos.driver.rtc.pl031/package.bexmanifest=$(location //drivers/d1/rtc/arm/pl031:package_manifest) ", "--entry /boot/pkg/bexos.driver.rtc.cmos/package.bexmanifest=$(location //drivers/d1/rtc/pc/cmos:package_manifest) ") +
              "--entry /boot/pkg/bexos.driver.storage.nvme/package.bexmanifest=$(location //drivers/d1/storage/nvmexpress/nvme:package_manifest) " +
              "--entry /boot/pkg/bexos.driver.storage.bexfs/package.bexmanifest=$(location //drivers/d1/storage/bexos/bexfs:package_manifest) " +
              "--entry /boot/pkg/bexos.driver.storage.user_bexfs/package.bexmanifest=$(location //drivers/d1/storage/bexos/bexfs:user_package_manifest) " +
              "--entry /boot/pkg/bexos.driver.storage.archivefs/package.bexmanifest=$(location //drivers/d1/storage/bexos/archivefs:package_manifest) " +
              "--entry /boot/pkg/bexos.driver.storage.memfs/package.bexmanifest=$(location //drivers/d1/storage/bexos/memfs:package_manifest) " +
              "--entry /boot/pkg/bexos.driver.storage.diskimage/package.bexmanifest=$(location //drivers/d1/storage/bexos/diskimage:package_manifest) " +
              "--entry /boot/pkg/bexos.service.vfsd/package.bexmanifest=$(location //services/vfsd:package_manifest) " +
              "--entry /boot/pkg/bexos.service.teed/package.bexmanifest=$(location //services/teed:package_manifest_software) " +
              "--entry /boot/pkg/bexos.driver.debugd/package.bexmanifest=$(location //services/debugd:package_manifest) " +
              "--entry /boot/pkg/bexos.service.traced/package.bexmanifest=$(location //services/traced:package_manifest) " +
              "--entry /boot/pkg/bexos.service.updated/package.bexmanifest=$(location //services/updated:package_manifest) " +
              "--entry /boot/pkg/bexos.service.trustd/package.bexmanifest=$(location //services/trustd:package_manifest) " +
              "--entry /boot/pkg/bexos.service.powerd/package.bexmanifest=$(location //services/powerd:package_manifest) " +
              "--entry /boot/pkg/bexos.service.usersd/package.bexmanifest=$(location //services/usersd:package_manifest) " +
              "--entry /boot/pkg/bexos.service.keychaind/package.bexmanifest=$(location //services/keychaind:package_manifest) " +
              "--entry /boot/pkg/bexos.lib.crypto/package.bexmanifest=$(location //lib/crypto:package_manifest) " +
              "--entry /boot/pkg/bexos.lib.crypto/lib/libbexos_crypto.so=$(location //lib/crypto:crypto_shared) " +
              "--entry /boot/pkg/bexos.lib.net/package.bexmanifest=$(location //lib/net:package_manifest) " +
              "--entry /boot/pkg/bexos.lib.net/lib/libbexos_net.so=$(location //lib/net:net_shared) " +
              "--entry /boot/pkg/bexos.lib.tee_driver.software/package.bexmanifest=$(location //lib/tee_driver_software:package_manifest) " +
              "--entry /boot/pkg/bexos.lib.tee_driver.software/lib/libbexos_tee_driver.so=$(location //lib/tee_driver_software:tee_driver_software_shared) " +
              "--entry /system/pkg/bexos.lib.tee_driver.trusty.bex=$(location //lib/tee_driver_trusty:tee_driver_trusty_archive) " +
              "--entry /system/pkg/bexos.lib.tee_driver.software.bex=$(location //lib/tee_driver_software:tee_driver_software_archive) " +
              "--entry /system/ecosystem/bexos.bundle=$(location //ecosystem/bexos:public_bundle) " +
              "--entry /system/certs/tls_roots.redb=$(location //ecosystem/bexos:tls_roots_redb) " +
              "--entry /system/certs/app_signing_roots.redb=$(location //ecosystem/bexos:app_signing_roots_redb) " +
              "--elf /boot/pkg/bexos.platform.appd/bin/appd=$(location //services/appd:appd_elf) " +
              "--entry /boot/pkg/bexos.platform.wasm_runner/bin/wasm_runner=$(location //services/wasm_runner:wasm_runner_elf) " +
              "--entry /boot/pkg/bexos.test.wasm.boot/package.bexmanifest=$(location //testing/wasm:boot_manifest) " +
              "--entry /boot/pkg/bexos.test.wasm.boot/bin/boot.wasm=$(location //testing/wasm:boot_wasm) " +
              "--elf /boot/pkg/bexos.driver.pci_root/bin/pci_root_bus=$(location //drivers/d1/bus/generic/pci:pci_root_bus) " +
              guest_select("--elf /boot/pkg/bexos.driver.uart.pl011/bin/pl011=$(location //drivers/d1/serial/arm/pl011:pl011) ", "") +
              guest_select("--elf /boot/pkg/bexos.driver.rtc.pl031/bin/pl031=$(location //drivers/d1/rtc/arm/pl031:pl031) ", "--elf /boot/pkg/bexos.driver.rtc.cmos/bin/cmos=$(location //drivers/d1/rtc/pc/cmos:cmos) ") +
              "--elf /boot/pkg/bexos.driver.storage.nvme/bin/nvme_driver=$(location //drivers/d1/storage/nvmexpress/nvme:nvme_driver) " +
              "--elf /boot/pkg/bexos.driver.storage.bexfs/bin/bexfs_driver=$(location //drivers/d1/storage/bexos/bexfs:bexfs_driver) " +
              "--elf /boot/pkg/bexos.driver.storage.user_bexfs/bin/user_bexfs_driver=$(location //drivers/d1/storage/bexos/bexfs:user_bexfs_driver) " +
              "--elf /boot/pkg/bexos.driver.storage.archivefs/bin/archivefs_driver=$(location //drivers/d1/storage/bexos/archivefs:archivefs_driver) " +
              "--elf /boot/pkg/bexos.driver.storage.memfs/bin/memfs_driver=$(location //drivers/d1/storage/bexos/memfs:memfs_driver) " +
              "--elf /boot/pkg/bexos.driver.storage.diskimage/bin/diskimage_driver=$(location //drivers/d1/storage/bexos/diskimage:diskimage_driver) " +
              "--elf /boot/pkg/bexos.service.vfsd/bin/vfsd=$(location //services/vfsd:vfsd_elf) " +
              "--elf /boot/pkg/bexos.service.teed/bin/teed=$(location //services/teed:teed_elf) " +
              "--elf /boot/pkg/bexos.driver.debugd/bin/debugd=$(location //services/debugd:debugd_elf) " +
              "--elf /boot/pkg/bexos.service.traced/bin/traced=$(location //services/traced:traced_elf) " +
              "--elf /boot/pkg/bexos.service.updated/bin/updated=$(location //services/updated:updated_elf) " +
              "--elf /boot/pkg/bexos.service.trustd/bin/trustd=$(location //services/trustd:trustd_elf) " +
              "--elf /boot/pkg/bexos.service.powerd/bin/powerd=$(location //services/powerd:powerd_elf) " +
              "--elf /boot/pkg/bexos.service.usersd/bin/usersd=$(location //services/usersd:usersd_elf) " +
              "--elf /boot/pkg/bexos.service.keychaind/bin/keychaind=$(location //services/keychaind:keychaind_elf)",
        tools = ["//tools/image:assemble_bootfs", "//tools/app_manifest:stamp"],
    )

    native.genrule(
        name = "boot_handoff",
        srcs = [":bootfs_image", ":device_prototxt_source", "//device/virtual/qemu/base:keys/avb_dev_private_key.hex", "//kernel:kernel_bin"],
        outs = ["boot_handoff.bin", "boot_layout.sh", "boot_evidence.bin"],
        cmd = "$(location //tools/image:boot_handoff) $(location :bootfs_image) $(location boot_handoff.bin) $(location boot_layout.sh) $(location :device_prototxt_source) $(location boot_evidence.bin) $(location //kernel:kernel_bin) $(location //device/virtual/qemu/base:keys/avb_dev_private_key.hex)" + guest_select("", " --secure-boot --rpmb --secure-monitor"),
        tools = ["//tools/image:boot_handoff"],
    )

    native.genrule(
        name = "boot_handoff_emulated",
        srcs = [":bootfs_image_emulated", ":device_emulated_prototxt_source", "//device/virtual/qemu/base:keys/avb_dev_private_key.hex", ":emulated_kernel"],
        outs = ["boot_handoff.emulated.bin", "boot_layout.emulated.sh", "boot_evidence.emulated.bin"],
        cmd = "$(location //tools/image:boot_handoff) $(location :bootfs_image_emulated) $(location boot_handoff.emulated.bin) $(location boot_layout.emulated.sh) $(location :device_emulated_prototxt_source) $(location boot_evidence.emulated.bin) $(location :emulated_kernel) $(location //device/virtual/qemu/base:keys/avb_dev_private_key.hex)",
        tools = ["//tools/image:boot_handoff"],
    )

    native.genrule(
        name = "qemu_sys_state_bexfs_image",
        srcs = ["//device/virtual/qemu/base:qemu_bexfs_test.key"],
        outs = ["sys_state.bexfs.img"],
        cmd = "$(location //tools/image:bexfs_image) " +
              "--out $@ --size-bytes 33554432 --label SYS_STATE " +
              "--volume-uuid 53595353-5441-5445-0000-000000000001 " +
              "--key-file $(location //device/virtual/qemu/base:qemu_bexfs_test.key) --initial-sys-state",
        tools = ["//tools/image:bexfs_image"],
    )

    native.genrule(
        name = "qemu_nvme_disk_image",
        srcs = [":qemu_sys_state_bexfs_image", ":qemu_storage"],
        outs = ["qemu_nvme_gpt.img"],
        cmd = "$(location //tools/image:generate_gpt_disk) --out $@ " +
              "--partition-image SYS_STATE=$(location :qemu_sys_state_bexfs_image) " +
              "--partition-image STORAGE=$(location :qemu_storage)",
        tools = ["//tools/image:generate_gpt_disk"],
    )

    native.genrule(
        name = "vbmeta",
        srcs = [
            ":bootfs_image",
            ":platform_config_bin",
            "//device/virtual/qemu/base:keys/avb_dev_rsa_private.pem",
            "//kernel:kernel_bin",
        ],
        outs = ["vbmeta.img", "avb_dev_public_key.bin"],
        cmd = "$(location //tools/image:vbmeta) " +
              "$(location //device/virtual/qemu/base:keys/avb_dev_rsa_private.pem) " +
              "$(location vbmeta.img) $(location avb_dev_public_key.bin) 2 " +
              "kernel=$(location //kernel:kernel_bin) " +
              "bootfs=$(location :bootfs_image) " +
              "platform-policy=$(location :platform_config_bin)",
        tools = ["//tools/image:vbmeta"],
    )

    native.genrule(
        name = "vbmeta_elf",
        srcs = [
            ":bootfs_image",
            ":platform_config_bin",
            "//device/virtual/qemu/base:keys/avb_dev_rsa_private.pem",
            "//kernel:kernel",
        ],
        outs = ["vbmeta.elf.img", "avb_dev_public_key.elf.bin"],
        cmd = "$(location //tools/image:vbmeta) " +
              "$(location //device/virtual/qemu/base:keys/avb_dev_rsa_private.pem) " +
              "$(location vbmeta.elf.img) $(location avb_dev_public_key.elf.bin) 2 " +
              "kernel=$(location //kernel:kernel) " +
              "bootfs=$(location :bootfs_image) " +
              "platform-policy=$(location :platform_config_bin)",
        tools = ["//tools/image:vbmeta"],
    )

    native.genrule(
        name = "vbmeta_emulated",
        srcs = [
            ":bootfs_image_emulated",
            ":platform_config_emulated_bin",
            "//device/virtual/qemu/base:keys/avb_dev_rsa_private.pem",
            ":emulated_kernel",
        ],
        outs = ["vbmeta.emulated.img", "avb_dev_public_key.emulated.bin"],
        cmd = "$(location //tools/image:vbmeta) " +
              "$(location //device/virtual/qemu/base:keys/avb_dev_rsa_private.pem) " +
              "$(location vbmeta.emulated.img) $(location avb_dev_public_key.emulated.bin) 2 " +
              "kernel=$(location :emulated_kernel) " +
              "bootfs=$(location :bootfs_image_emulated) " +
              "platform-policy=$(location :platform_config_emulated_bin)",
        tools = ["//tools/image:vbmeta"],
    )

    native.genrule(
        name = "vbmeta_stale",
        srcs = [
            ":bootfs_image",
            ":platform_config_bin",
            "//device/virtual/qemu/base:keys/avb_dev_rsa_private.pem",
            "//kernel:kernel_bin",
        ],
        outs = ["vbmeta.stale.img", "avb_dev_public_key.stale.bin"],
        cmd = "$(location //tools/image:vbmeta) " +
              "$(location //device/virtual/qemu/base:keys/avb_dev_rsa_private.pem) " +
              "$(location vbmeta.stale.img) $(location avb_dev_public_key.stale.bin) 1 " +
              "kernel=$(location //kernel:kernel_bin) " +
              "bootfs=$(location :bootfs_image) " +
              "platform-policy=$(location :platform_config_bin)",
        tools = ["//tools/image:vbmeta"],
    )

    app_archive(
        name = "storage_verify_archive",
        manifest = "//services/storage_verify:manifest",
        entries = {
            "bin/storage_verify": "//services/storage_verify:storage_verify",
        },
        config = ":storage_verify_component_config",
        compression = "none",
    )

    app_archive(
        name = "virtio_net_archive",
        manifest = "//drivers/d1/nic/virtio/net:package_manifest",
        entries = {
            "bin/virtio_net_driver": "//drivers/d1/nic/virtio/net:virtio_net_driver",
        },
        compression = "none",
    )

    app_archive(
        name = "qemu_netstackd_archive",
        manifest = "//services/netstack:package_manifest",
        entries = {
            "bin/netstackd": "//services/netstack:netstackd_elf",
        },
        config = ":netstackd_component_config",
        compression = "none",
    )

    app_archive(
        name = "qemu_timed_archive",
        manifest = "//services/timed:package_manifest",
        entries = {
            "bin/timed": "//services/timed:timed_elf",
        },
        config = ":timed_component_config",
        compression = "none",
    )

    native.genrule(
        name = "qemu_storage",
        srcs = ["//device/virtual/qemu/base:pkg.keep", "//device/virtual/qemu/base:qemu_bexfs_test.key", "//device/virtual/qemu/base:storage_data.keep"] +
               [entry["archive"] for entry in storage_preinstalls],
        outs = ["storage.bexfs.img"],
        # BexFS keeps two full snapshots. The local font baseline and the
        # heart-transplant archives must fit in both, with headroom for user
        # font installs and other runtime writes. The shared size matches the
        # STORAGE partition in generate_gpt_disk.py.
        cmd = ("$(location //tools/image:bexfs_image) --out $@ --size-bytes %d --label STORAGE " % QEMU_STORAGE_SIZE_BYTES) +
              "--volume-uuid 53544f52-4147-4500-0000-000000000004 --key-file $(location //device/virtual/qemu/base:qemu_bexfs_test.key) " +
              "--entry pkg/.keep=$(location //device/virtual/qemu/base:pkg.keep) " +
              "".join([
                  "--entry %s=$(location %s) " % (entry["path"], entry["archive"])
                  for entry in storage_preinstalls
              ]) +
              "--entry data/.keep=$(location //device/virtual/qemu/base:storage_data.keep)",
        tools = ["//tools/image:bexfs_image"],
    )

    native.genrule(
        name = "qemu_nvme_development_disk_image",
        srcs = [":qemu_nvme_gpt.img", "//device/virtual/qemu/base:qemu_bexfs_test.key", "//lib/tee_driver_software:tee_driver_software_archive"],
        outs = ["qemu_nvme_development_gpt.img"],
        cmd = "cp $(location :qemu_nvme_gpt.img) $@ && chmod u+w $@ && " +
              "$(location //tools/image:bexfs_image) --inspect --image $@ --partition STORAGE --label STORAGE " +
              "--key-file $(location //device/virtual/qemu/base:qemu_bexfs_test.key) " +
              "--entry pkg/bexos.lib.tee_driver.software.bex=$(location //lib/tee_driver_software:tee_driver_software_archive)",
        tools = ["//tools/image:bexfs_image"],
    )

    native.alias(name = "emulated_kernel", actual = guest_select("//kernel:kernel", "//kernel:kernel_bin"))

    native.genrule(
        name = "qemu_nvme_without_brush_disk_image",
        srcs = [":qemu_sys_state_bexfs_image", ":qemu_storage_without_brush"],
        outs = ["qemu_nvme_without_brush_gpt.img"],
        cmd = "$(location //tools/image:generate_gpt_disk) --out $@ " +
              "--partition-image SYS_STATE=$(location :qemu_sys_state_bexfs_image) " +
              "--partition-image STORAGE=$(location :qemu_storage_without_brush)",
        tools = ["//tools/image:generate_gpt_disk"],
    )

    native.genrule(
        name = "qemu_nvme_shell_fixture_disk_image",
        srcs = [":qemu_sys_state_bexfs_image", ":qemu_storage_shell_fixture"],
        outs = ["qemu_nvme_shell_fixture_gpt.img"],
        cmd = "$(location //tools/image:generate_gpt_disk) --out $@ " +
              "--partition-image SYS_STATE=$(location :qemu_sys_state_bexfs_image) " +
              "--partition-image STORAGE=$(location :qemu_storage_shell_fixture)",
        tools = ["//tools/image:generate_gpt_disk"],
    )

    native.genrule(
        name = "qemu_storage_without_brush",
        srcs = ["//device/virtual/qemu/base:pkg.keep", "//device/virtual/qemu/base:qemu_bexfs_test.key", "//device/virtual/qemu/base:storage_data.keep"] +
               [entry["archive"] for entry in QEMU_STORAGE_PREINSTALLS_WITHOUT_BRUSH],
        outs = ["storage.without_brush.bexfs.img"],
        cmd = ("$(location //tools/image:bexfs_image) --out $@ --size-bytes %d --label STORAGE " % QEMU_STORAGE_SIZE_BYTES) +
              "--volume-uuid 53544f52-4147-4500-0000-000000000004 --key-file $(location //device/virtual/qemu/base:qemu_bexfs_test.key) " +
              "--entry pkg/.keep=$(location //device/virtual/qemu/base:pkg.keep) " +
              "".join([
                  "--entry %s=$(location %s) " % (entry["path"], entry["archive"])
                  for entry in QEMU_STORAGE_PREINSTALLS_WITHOUT_BRUSH
              ]) +
              "--entry data/.keep=$(location //device/virtual/qemu/base:storage_data.keep)",
        tools = ["//tools/image:bexfs_image"],
    )

    native.genrule(
        name = "qemu_storage_shell_fixture",
        srcs = ["//device/virtual/qemu/base:pkg.keep", "//device/virtual/qemu/base:qemu_bexfs_test.key", "//device/virtual/qemu/base:storage_data.keep"] +
               [entry["archive"] for entry in QEMU_STORAGE_PREINSTALLS_SHELL_FIXTURE],
        outs = ["storage.shell_fixture.bexfs.img"],
        cmd = ("$(location //tools/image:bexfs_image) --out $@ --size-bytes %d --label STORAGE " % QEMU_STORAGE_SIZE_BYTES) +
              "--volume-uuid 53544f52-4147-4500-0000-000000000004 --key-file $(location //device/virtual/qemu/base:qemu_bexfs_test.key) " +
              "--entry pkg/.keep=$(location //device/virtual/qemu/base:pkg.keep) " +
              "".join([
                  "--entry %s=$(location %s) " % (entry["path"], entry["archive"])
                  for entry in QEMU_STORAGE_PREINSTALLS_SHELL_FIXTURE
              ]) +
              "--entry data/.keep=$(location //device/virtual/qemu/base:storage_data.keep)",
        tools = ["//tools/image:bexfs_image"],
    )
