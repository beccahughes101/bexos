"""Shared QEMU product configuration and package assembly."""
load("//build/platforms:architecture.bzl", "guest_select")
load("//build/rules:assembly.bzl", "assembly_input_bundle", "product_app_config", "starlark_product")

QEMU_PRODUCT_MANIFESTS = {
        "//testing/e2e/qemu/graphics/input_fixture:fixture_elf": "//testing/e2e/qemu/graphics/input_fixture:package_manifest",
        "//drivers/d1/input/virtio:input_driver": "//drivers/d1/input/virtio:package_manifest",
        "//drivers/d1/display/virtio/gpu:gpu_driver": "//drivers/d1/display/virtio/gpu:package_manifest",
        "//services/splashd:splashd_elf": "//services/splashd:package_manifest",
        "//services/scened:scened_elf": "//services/scened:package_manifest",
        "//drivers/d1/rtc/pc/cmos:cmos": "//drivers/d1/rtc/pc/cmos:package_manifest",
        "//apps/brush_shell:brush_shell": "//apps/brush_shell:manifest",
        "//apps/dioxus_shared:dioxus_shared": "//apps/dioxus_shared:manifest",
        "//apps/dioxus_demo:dioxus_demo": "//apps/dioxus_demo:manifest",
        "//apps/sysui:sysui": "//apps/sysui:manifest",
        "//apps/userui:userui": "//apps/userui:manifest",
        "//testing/wasm:boot_wasm": "//testing/wasm:boot_manifest",
        "//testing/wasm:file_service_archive": "//testing/wasm:file_service_manifest",
        "//testing/wasm:file_client_archive": "//testing/wasm:file_client_manifest",
        "//testing/wasm:memory_archive": "//testing/wasm:memory_manifest",
        "//testing/wasm:network_denied_archive": "//testing/wasm:network_denied_manifest",
        "//testing/wasm:network_granted_archive": "//testing/wasm:network_granted_manifest",
        "//testing/wasm:children_archive": "//testing/wasm:children_manifest",
        "//testing/wasm:client_archive": "//testing/wasm:client_manifest",
        "//testing/wasm:command_archive": "//testing/wasm:command_manifest",
        "//testing/wasm:trap_archive": "//testing/wasm:trap_manifest",
        "//testing/wasm:service_archive": "//testing/wasm:service_manifest",

        "//drivers/d1/serial/virtio/console:virtio_console_driver": "//drivers/d1/serial/virtio/console:package_manifest",
        "//drivers/d1/storage/bexos/archivefs:archivefs_driver": "//drivers/d1/storage/bexos/archivefs:package_manifest",
        "//drivers/d1/storage/bexos/bexfs:bexfs_driver": "//drivers/d1/storage/bexos/bexfs:package_manifest",
        "//drivers/d1/storage/bexos/bexfs:user_bexfs_driver": "//drivers/d1/storage/bexos/bexfs:user_package_manifest",
        "//drivers/d1/storage/bexos/diskimage:diskimage_driver": "//drivers/d1/storage/bexos/diskimage:package_manifest",
        "//drivers/d1/storage/bexos/memfs:memfs_driver": "//drivers/d1/storage/bexos/memfs:package_manifest",
        "//drivers/d1/storage/nvmexpress/nvme:nvme_driver": "//drivers/d1/storage/nvmexpress/nvme:package_manifest",
        "//drivers/d1/storage/usb/bot:usb_bot": "//drivers/d1/storage/usb/bot:package_manifest",
        "//drivers/d1/bus/generic/pci:pci_root_bus": "//drivers/d1/bus/generic/pci:package_manifest",
        "//drivers/d1/usb/xhci:xhcid": "//drivers/d1/usb/xhci:package_manifest",
        "//drivers/d1/input/usb_hid:usb_hid": "//drivers/d1/input/usb_hid:package_manifest",
        "//drivers/d1/rtc/arm/pl031:pl031": "//drivers/d1/rtc/arm/pl031:package_manifest",
        "//drivers/d1/serial/arm/pl011:pl011": "//drivers/d1/serial/arm/pl011:package_manifest",
        "//drivers/d1/nic/virtio/net:virtio_net_driver": "//drivers/d1/nic/virtio/net:package_manifest",
        "//lib/crypto:crypto_archive": "//lib/crypto:package_manifest",
        "//lib/net:net_archive": "//lib/net:package_manifest",
        "//lib/tee_driver_software:tee_driver_software_archive": "//lib/tee_driver_software:package_manifest",
        "//lib/tee_driver_trusty:tee_driver_trusty_archive": "//lib/tee_driver_trusty:package_manifest",
        "//lib/ui/theme:theme_archive": "//lib/ui/theme:manifest",
        "//services/appd:appd_elf": "//services/appd:package_manifest",
        "//services/usbd:usbd": "//services/usbd:package_manifest",
        "//services/wasm_runner:wasm_runner_archive": "//services/wasm_runner:package_manifest",
        "//services/debugd:debugd_elf": "//services/debugd:package_manifest",
        "//services/keychaind:keychaind_elf": "//services/keychaind:package_manifest",
        "//services/prefsd:prefsd_archive": "//services/prefsd:package_manifest",
        "//services/jobd:jobd_archive": "//services/jobd:package_manifest",
        "//services/netstack:netstackd_archive": "//services/netstack:package_manifest",
        "//services/powerd:powerd_elf": "//services/powerd:package_manifest",
        "//services/teed:teed_elf": "//services/teed:package_manifest",
        "//services/storage_verify:storage_verify": "//services/storage_verify:manifest",
        "//services/timed:timed_archive": "//services/timed:package_manifest",
        "//services/traced:traced_elf": "//services/traced:package_manifest",
        "//services/trustd:trustd_elf": "//services/trustd:package_manifest",
        "//services/updated:updated_elf": "//services/updated:package_manifest",
        "//services/usersd:usersd_elf": "//services/usersd:package_manifest",
        "//services/vfsd:vfsd_elf": "//services/vfsd:package_manifest",
    }

def qemu_assembly(product, graphics, input_fixture = False):
    native.filegroup(
        name = "config_sources",
        srcs = [
            ":bootfs_manifest_prototxt_source",
            ":qemu_hardware_aib_prototxt_source",
            ":device_emulated_prototxt_source",
            ":device_prototxt_source",
            ":platform_pcfg_prototxt_source",
            ":platform_emulated_prototxt_source",
            "products.star",
            ":system_image_prototxt_source",
        ],
    )

    assembly_input_bundle(
        name = "qemu_hardware_aib",
        src = ":qemu_hardware_aib_prototxt_source",
    )

    assembly_input_bundle(name = "aarch64_hardware_aib", src = "//device/virtual/qemu/base/aarch64:qemu_hardware.aib.prototxt")

    assembly_input_bundle(name = "x86_64_hardware_aib", src = "//device/virtual/qemu/base/x86_64:qemu_hardware.aib.prototxt")

    starlark_product(
        name = "virtual_aarch64_product_assembly",
        src = "products.star",
        loads = {
            "//device/base:configs.star": "//device/base:configs.star",
            "//device/virtual/qemu/base:configs.star": "//device/virtual/qemu/base:configs.star",
        },
        entry = "virtual_aarch64",
        bundles = ["//device/base:base", ":aarch64_hardware_aib_bin"] + (["//device/base/graphics:graphics", "//device/virtual/qemu/base/graphics:qemu_graphics"] if graphics else []) + (["//testing/e2e/qemu/graphics/input_fixture:bundle_bin"] if input_fixture else []),
        manifests = {label: manifest for label, manifest in QEMU_PRODUCT_MANIFESTS.items() if "/pc/cmos" not in label},
    )

    starlark_product(
        name = "virtual_x86_64_product_assembly",
        src = "products.star",
        loads = {
            "//device/base:configs.star": "//device/base:configs.star",
            "//device/virtual/qemu/base:configs.star": "//device/virtual/qemu/base:configs.star",
        },
        entry = "virtual_x86_64",
        bundles = ["//device/base:base", ":x86_64_hardware_aib_bin"] + (["//device/base/graphics:graphics", "//device/virtual/qemu/base/graphics:qemu_graphics"] if graphics else []) + (["//testing/e2e/qemu/graphics/input_fixture:bundle_bin"] if input_fixture else []),
        manifests = {label: manifest for label, manifest in QEMU_PRODUCT_MANIFESTS.items() if "/arm/" not in label},
    )

    starlark_product(
        name = "virtual_x86_64_development_product_assembly",
        src = "products.star",
        loads = {
            "//device/base:configs.star": "//device/base:configs.star",
            "//device/virtual/qemu/base:configs.star": "//device/virtual/qemu/base:configs.star",
        },
        entry = "virtual_x86_64_development",
        bundles = ["//device/base:base", ":x86_64_hardware_aib_bin"] + (["//device/base/graphics:graphics", "//device/virtual/qemu/base/graphics:qemu_graphics"] if graphics else []) + (["//testing/e2e/qemu/graphics/input_fixture:bundle_bin"] if input_fixture else []),
        manifests = {label: ("//services/teed:package_manifest_software" if label == "//services/teed:teed_elf" else manifest) for label, manifest in QEMU_PRODUCT_MANIFESTS.items() if "/arm/" not in label},
    )

    product_app_config(
        name = "storage_verify_component_config",
        manifest = "//services/storage_verify:manifest",
        package_id = "bexos.platform.storage_verify",
        product = ":product_definition",
    )

    product_app_config(
        name = "netstackd_component_config",
        manifest = "//services/netstack:package_manifest",
        package_id = "bexos.service.netstackd",
        product = ":product_definition",
    )

    product_app_config(
        name = "timed_component_config",
        manifest = "//services/timed:package_manifest",
        package_id = "bexos.service.timed",
        product = ":product_definition",
    )

    native.filegroup(
        name = "virtual_aarch64_product_assembly_index",
        srcs = ["virtual_aarch64_product_assembly.assembly"],
    )

    native.genrule(
        name = "bootfs_manifest_validation",
        srcs = [
            ":bootfs_manifest_prototxt_source",
            ":product_bootfs_labels",
            "//testing/wasm:boot_wasm",
            "//drivers/d1/bus/generic/pci:pci_root_bus",
            "//drivers/d1/storage/nvmexpress/nvme:nvme_driver",
            "//drivers/d1/storage/bexos/bexfs:bexfs_driver",
            "//drivers/d1/storage/bexos/bexfs:user_bexfs_driver",
            "//drivers/d1/storage/bexos/archivefs:archivefs_driver",
            "//drivers/d1/storage/bexos/memfs:memfs_driver",
            "//drivers/d1/storage/bexos/diskimage:diskimage_driver",
            "//drivers/d1/storage/usb/bot:usb_bot",
            "//drivers/d1/usb/xhci:xhcid",
            "//drivers/d1/input/usb_hid:usb_hid",
            "//services/vfsd:vfsd_elf",
            "//services/debugd:debugd_elf",
            "//services/traced:traced_elf",
            "//services/updated:updated_elf",
            "//services/trustd:trustd_elf",
            "//services/teed:teed_elf",
            "//services/powerd:powerd_elf",
            "//services/usersd:usersd_elf",
            "//services/keychaind:keychaind_elf",
            "//services/appd:appd_elf",
            "//services/usbd:usbd",
        ] + guest_select(["//drivers/d1/serial/arm/pl011:pl011", "//drivers/d1/rtc/arm/pl031:pl031"], ["//drivers/d1/rtc/pc/cmos:cmos", "//drivers/d1/rtc/pc/cmos:package_manifest"]),
        outs = ["bootfs_manifest.validation"],
        cmd = "$(location //tools/image:validate_bootfs_manifest) --manifest $(location :bootfs_manifest_prototxt_source) --declared-labels-file $(location :product_bootfs_labels) --stamp $@",
        tools = ["//tools/image:validate_bootfs_manifest"],
    )

    native.genrule(
        name = "device_bin",
        srcs = [
            ":device_prototxt_source",
            "//idl:platform_config_proto_src",
        ],
        outs = ["device.bin"],
        cmd = "$(location @protobuf//:protoc) --proto_path=. --encode=bexos.platform.Device idl/bexos/platform/device.proto < $(location :device_prototxt_source) > $@",
        tools = ["@protobuf//:protoc"],
    )

    native.genrule(
        name = "device_emulated_bin",
        srcs = [
            ":device_emulated_prototxt_source",
            "//idl:platform_config_proto_src",
        ],
        outs = ["device.emulated.bin"],
        cmd = "$(location @protobuf//:protoc) --proto_path=. --encode=bexos.platform.Device idl/bexos/platform/device.proto < $(location :device_emulated_prototxt_source) > $@",
        tools = ["@protobuf//:protoc"],
    )

    native.sh_test(
        name = "device_secure_world_test",
        srcs = ["//device/virtual/qemu/base:validate_device_secure_world.sh"],
        args = [
            "$(location @protobuf//:protoc)",
            ".",
            "idl/bexos/platform/device.proto",
            "$(location :device_bin)",
            "$(location :device_emulated_bin)",
            "$(location :device_prototxt_source)",
        ] + guest_select(["aarch64"], ["x86_64"]),
        data = [
            ":device_bin",
            ":device_emulated_bin",
            ":device_prototxt_source",
            "//idl:platform_config_proto_src",
            "@protobuf//:protoc",
        ],
    )

    native.genrule(
        name = "bootfs_manifest_bin",
        srcs = [
            ":bootfs_manifest_prototxt_source",
            ":bootfs_manifest_validation",
            "//idl:platform_config_proto_src",
        ],
        outs = ["bootfs_manifest.bin"],
        cmd = "$(location @protobuf//:protoc) --proto_path=. --encode=bexos.platform.BootfsManifest idl/bexos/platform/device.proto < $(location :bootfs_manifest_prototxt_source) > $@",
        tools = ["@protobuf//:protoc"],
    )

    native.genrule(
        name = "system_image_bin",
        srcs = [
            ":system_image_prototxt_source",
            "//idl:platform_config_proto_src",
        ],
        outs = ["system_image.bin"],
        cmd = "$(location @protobuf//:protoc) --proto_path=. --encode=bexos.platform.SystemImageManifest idl/bexos/platform/device.proto < $(location :system_image_prototxt_source) > $@",
        tools = ["@protobuf//:protoc"],
    )

    native.genrule(
        name = "platform_config_bin",
        srcs = [
            ":platform_pcfg_prototxt_source",
            "//idl:platform_config_proto_src",
        ],
        outs = ["platform.pcfg"],
        cmd = "$(location @protobuf//:protoc) --proto_path=. --encode=bexos.platform.PlatformConfig idl/bexos/platform/config.proto < $(location :platform_pcfg_prototxt_source) > $@",
        tools = ["@protobuf//:protoc"],
    )

    native.genrule(
        name = "platform_config_emulated_bin",
        srcs = [
            ":platform_emulated_prototxt_source",
            "//idl:platform_config_proto_src",
        ],
        outs = ["platform.emulated.pcfg"],
        cmd = "$(location @protobuf//:protoc) --proto_path=. --encode=bexos.platform.PlatformConfig idl/bexos/platform/config.proto < $(location :platform_emulated_prototxt_source) > $@",
        tools = ["@protobuf//:protoc"],
    )

    native.alias(name = "device_prototxt_source", actual = guest_select("//device/virtual/qemu/base/aarch64:device.prototxt", "//device/virtual/qemu/base/x86_64:device.prototxt"))

    native.genrule(
        name = "x86_64_verified_platform_policy",
        srcs = ["//device/virtual/qemu/base/x86_64:platform.pcfg.prototxt", "//third_party/trusty:product_x86/lk.elf", "//tools/image:stamp_firmware_policy.py"],
        outs = ["x86_64/platform.verified.prototxt"],
        cmd = "python3 -B $(location //tools/image:stamp_firmware_policy.py) $(location //device/virtual/qemu/base/x86_64:platform.pcfg.prototxt) $(location //third_party/trusty:product_x86/lk.elf) $@",
    )
    native.alias(name = "platform_pcfg_prototxt_source", actual = guest_select("//device/virtual/qemu/base/aarch64:platform.pcfg.prototxt", ":x86_64_verified_platform_policy"))

    native.alias(name = "bootfs_manifest_prototxt_source", actual = guest_select("//device/virtual/qemu/base/aarch64:bootfs_manifest.prototxt", "//device/virtual/qemu/base/x86_64:bootfs_manifest.prototxt"))

    native.alias(name = "qemu_hardware_aib_prototxt_source", actual = guest_select("//device/virtual/qemu/base/aarch64:qemu_hardware.aib.prototxt", "//device/virtual/qemu/base/x86_64:qemu_hardware.aib.prototxt"))

    native.alias(name = "system_image_prototxt_source", actual = guest_select("//device/virtual/qemu/base/aarch64:system_image.prototxt", "//device/virtual/qemu/base/x86_64:system_image.prototxt"))

    native.alias(name = "product_definition", actual = guest_select(":virtual_aarch64_product_assembly_definition_bin", ":virtual_x86_64_product_assembly_definition_bin"))

    native.alias(name = "product_assembly_index", actual = guest_select(":virtual_aarch64_product_assembly.assembly", ":virtual_x86_64_product_assembly.assembly"))

    native.alias(name = "product_bootfs_labels", actual = guest_select(":virtual_aarch64_product_assembly.bootfs.labels", ":virtual_x86_64_product_assembly.bootfs.labels"))

    native.sh_test(
        name = "product_config_test",
        srcs = ["//device/virtual/qemu/base:validate_product.sh"],
        args = ["$(location @protobuf//:protoc)", product] + guest_select(["aarch64"], ["x86_64"]) + [
            "$(location :product_definition)",
            "$(location :platform_config_bin)",
            "$(location :product_assembly_index)",
            "$(locations @protobuf//:well_known_type_protos)",
        ],
        data = [
            "@protobuf//:protoc",
            "//idl:platform_config_proto_src",
            "//idl:app_manifest_proto_src",
            "@protobuf//:well_known_type_protos",
            ":product_definition",
            ":platform_config_bin",
            ":product_assembly_index",
        ],
    )

    native.alias(name = "device_emulated_prototxt_source", actual = guest_select("//device/virtual/qemu/base/aarch64:device.emulated.prototxt", "//device/virtual/qemu/base/x86_64:device.development.prototxt"))
    native.alias(name = "platform_emulated_prototxt_source", actual = guest_select("//device/virtual/qemu/base/aarch64:platform.emulated.pcfg.prototxt", "//device/virtual/qemu/base/x86_64:platform.development.pcfg.prototxt"))
    native.alias(name = "product_emulated_assembly_index", actual = guest_select(":virtual_aarch64_product_assembly.assembly", ":virtual_x86_64_development_product_assembly.assembly"))
