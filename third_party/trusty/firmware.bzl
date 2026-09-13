"""Source firmware builds, independent of saved-image consumers."""

def trusty_firmware(name, output_dir = "built", acceptance = False, architecture = "aarch64", generation_config = None):
    x86 = architecture == "x86_64"
    if generation_config and not x86:
        fail("replacement generation configuration is x86-only")
    x86_sources = ([":x86_runtime_sources"] + ([":x86_acceptance_sources"] if acceptance else [])) if x86 else []
    native.genrule(
        name = name,
        tags = ["manual"],
        srcs = ([":acceptance_sources", "acceptance/main.rs", "build_acceptance.py"] if acceptance else []) + ([generation_config] if generation_config else []) + x86_sources + (["configure_x86.py"] if x86 else ["//boot/bl33:verifier_bin", "//boot/bl33:rollback_provisioner_bin", "patches/qemu-gicv2-ackctl.patch"]) + [
            "//secure/orchestrator/trusty:all_sources",
            "@trusty_src//:all_sources",
            "@trusty_src//:BUILD.bazel",
            "@trusty_src//external/dtc:all_sources",
            "@rust_analyzer_1.80.1_tools//lib/rustlib/src:rustc_srcs",
            "@trusty_rust_compiler_builtins//:srcs",
            "@trusty_rust_hashbrown//:srcs",
            "@trusty_rust_rustc_demangle//:srcs",
            "authmgr_trusty_storage.rs",
            "authmgr_storage_format.rs",
            "binder_fd_compat.rs",
            "build_firmware.sh",
            "target_compatibility.sh",
            "host_prerequisites.sh",
            "reverse_lines.py",
            ":firmware_overlays",
            "patches/rust-1.80-trusty.patch",
            "patches/tf-a-qemu-trusty-memory.patch",
            "@trusty_compiler_rt_sources//:builtins_sources",
            "@trusty_compiler_rt_sources//:tf_marker",
            "@trusty_mbedtls_sources//:all_sources",
            "@trusty_mbedtls_sources//:version_marker",
            "//device/virtual/qemu/base:keys/avb_dev_rsa_private.pem",
        ],
        tools = [
            "@llvm//tools:clang",
            "@llvm//tools:clang++",
            "@llvm//tools:ld.lld",
            "@llvm//tools:llvm-ar",
            "@llvm//tools:llvm-cxxfilt",
            "@llvm//tools:llvm-nm",
            "@llvm//tools:llvm-objcopy",
            "@llvm//tools:llvm-objdump",
            "@llvm//tools:llvm-ranlib",
            "@llvm//tools:llvm-readelf",
            "@llvm//tools:llvm-size",
            "@llvm//tools:llvm-strip",
            "@protobuf//:protoc",
            ":trusty_bindgen",
            ":host_rpmb_dev",
            ":host_aidl",
            ":host_environment",
            "@trusty_host_libclang//:libclang",
            "@trusty_rust_1_80_1//:rustc",
            "@trusty_rust_1_80_1//:rustc_lib",
            ":host_rust_std",
            "@trusty_rust_1_80_1//:rustdoc",
            ":host_rustfmt",
        ],
        outs = [output_dir + "/" + artifact for artifact in (["lk.bin", "lk.elf", "rpmb_dev", "RPMB_DATA"] if x86 else [
            "bl1.bin",
            "bl2.bin",
            "bl31.bin",
            "lk.bin",
            "lk.elf",
            "bl33.bin",
            "bl33-rollback.bin",
            "rollback_nt_fw_content.crt",
            "nt_fw_content.crt",
            "nt_fw_key.crt",
            "rpmb_dev",
            "RPMB_DATA",
            "soc_fw_content.crt",
            "soc_fw_key.crt",
            "tb_fw.crt",
            "tos_fw_content.crt",
            "tos_fw_key.crt",
            "trusted_key.crt",
        ])],
        cmd = ("""
set -eu
. "$(location :host_environment)"
WORK="$(@D)/%OUTPUT_DIR%-input"
mkdir -p "$$WORK/source" "$$WORK/overlay"
TRUSTY_ROOT="$$(dirname "$(location @trusty_src//:BUILD.bazel)")"
# Upstream includes dangling editor/test-fixture symlinks; dereference declared
# source files while preserving the build's failure on missing compiler inputs.
python3 -c 'import os,shutil,sys; shutil.copytree(sys.argv[1], sys.argv[2], dirs_exist_ok=True, ignore=lambda directory,names: [name for name in names if name == ".git" or (os.path.islink(os.path.join(directory,name)) and not os.path.exists(os.path.join(directory,name)))])' "$$TRUSTY_ROOT" "$$WORK/source"
for src in $(locations //secure/orchestrator/trusty:all_sources); do
  rel="$${src#*secure/orchestrator/trusty/}"
  mkdir -p "$$WORK/overlay/$$(dirname "$$rel")"
  cp "$$src" "$$WORK/overlay/$$rel"
done
cp "$(location binder_fd_compat.rs)" \
  "$$WORK/source/frameworks/native/libs/binder/rust/src/fd_compat.rs"
cp "$(location authmgr_trusty_storage.rs)" \
  "$$WORK/source/trusty/user/app/authmgr/authmgr-be/lib/src/trusty_storage.rs"
cp "$(location authmgr_storage_format.rs)" \
  "$$WORK/source/trusty/user/app/authmgr/authmgr-be/lib/src/authmgr_storage_format.rs"
for src in $(locations :firmware_overlays); do
  rel="$${src#*third_party/trusty/overlays/}"
  mkdir -p "$$WORK/source/$$(dirname "$$rel")"
  cp "$$src" "$$WORK/source/$$rel"
done
for src in %X86_SOURCES%; do
  rel="$${src#*third_party/trusty/x86/}"
  mkdir -p "$$WORK/source/trusty/device/x86/bexos/$$(dirname "$$rel")"
  cp "$$src" "$$WORK/source/trusty/device/x86/bexos/$$rel"
done
RUST_SOURCES="$$WORK/rust-src"
mkdir -p "$$RUST_SOURCES"
for src in $(locations @rust_analyzer_1.80.1_tools//lib/rustlib/src:rustc_srcs); do
  rel="$${src#*/lib/rustlib/src/}"
  mkdir -p "$$RUST_SOURCES/$$(dirname "$$rel")"
  cp "$$src" "$$RUST_SOURCES/$$rel"
done
for vendor in compiler_builtins hashbrown rustc-demangle; do
  case "$$vendor" in
    compiler_builtins) vendor_srcs="$(locations @trusty_rust_compiler_builtins//:srcs)" ;;
    hashbrown) vendor_srcs="$(locations @trusty_rust_hashbrown//:srcs)" ;;
    rustc-demangle) vendor_srcs="$(locations @trusty_rust_rustc_demangle//:srcs)" ;;
  esac
  for src in $$vendor_srcs; do
    rel="$${src#*trusty_rust_$${vendor//-/_}/}"
    mkdir -p "$$RUST_SOURCES/vendor/$$vendor/$$(dirname "$$rel")"
    cp "$$src" "$$RUST_SOURCES/vendor/$$vendor/$$rel"
  done
done
manifest="$$WORK/overlay/manifest.prototxt"
uuid="$$(grep '^uuid:' "$$manifest" | cut -d: -f2 | tr -d ' \"')"
name="$$(grep '^app_name:' "$$manifest" | cut -d: -f2 | tr -d ' \"')"
heap="$$(grep '^min_heap:' "$$manifest" | tr -dc '0-9')"
stack="$$(grep '^min_stack:' "$$manifest" | tr -dc '0-9')"
printf '{\\n  "uuid": "%s",\\n  "app_name": "%s",\\n  "min_heap": %s,\\n  "min_stack": %s\\n}\\n' "$$uuid" "$$name" "$$heap" "$$stack" > "$$WORK/overlay/manifest.json"
BEXOS_TRUSTY_X86_GENERATION_CONFIG="%X86_GENERATION%" BEXOS_TRUSTY_ROLLBACK_VERIFIER="%ROLLBACK%" BEXOS_TRUSTY_ARCH="%ARCH%" BEXOS_TRUSTY_X86_CONFIG="%X86_CONFIG%" bash "$(location build_firmware.sh)" \
  "$$WORK/source" \
  "$$WORK/overlay" \
  "$(@D)/built" \
  "$(location @llvm//tools:clang)" \
  "$(location @llvm//tools:clang++)" \
  "$(location @llvm//tools:ld.lld)" \
  "$(location @llvm//tools:llvm-ar)" \
  "$(location @llvm//tools:llvm-cxxfilt)" \
  "$(location @llvm//tools:llvm-nm)" \
  "$(location @llvm//tools:llvm-objcopy)" \
  "$(location @llvm//tools:llvm-objdump)" \
  "$(location @llvm//tools:llvm-ranlib)" \
  "$(location @llvm//tools:llvm-readelf)" \
  "$(location @llvm//tools:llvm-size)" \
  "$(location @llvm//tools:llvm-strip)" \
  "$(location @protobuf//:protoc)" \
  "$(location :trusty_bindgen)" \
  "$(location @trusty_host_libclang//:libclang)" \
  "$(location @trusty_rust_1_80_1//:rustc)" \
  "$(location :host_rustfmt)" \
        "$$RUST_SOURCES/library/core/src/lib.rs" \
        "$(location //boot/bl33:verifier_bin)" \
        "$(location //device/virtual/qemu/base:keys/avb_dev_rsa_private.pem)" \
        "$(location @trusty_compiler_rt_sources//:tf_marker)" \
        "$(location @trusty_mbedtls_sources//:version_marker)" \
        "$(location @trusty_rust_1_80_1//:rustdoc)" \
        "$(location patches/rust-1.80-trusty.patch)" \
        "$(location patches/tf-a-qemu-trusty-memory.patch)" \
        "$$TRUSTY_MACOS_SDK_MARKER" \
        "$(location :host_rpmb_dev)" \
        "$(location :host_aidl)" \
        "$(location target_compatibility.sh)" \
        "$(location host_prerequisites.sh)" \
        "$(location reverse_lines.py)"
""").replace('cp "$(location binder_fd_compat.rs)"', ('patch -d "$$WORK/source" -p1 < "$(location patches/qemu-gicv2-ackctl.patch)"\n' if not x86 else "") + 'cp "$(location binder_fd_compat.rs)"').replace("$(@D)/built", "$(@D)/" + output_dir).replace(
            'bash "$(location build_firmware.sh)"',
            ('python3 "$(location build_acceptance.py)" "$(location build_firmware.sh)" "$(location acceptance/main.rs)"' if acceptance else 'bash "$(location build_firmware.sh)"'),
        ).replace("%OUTPUT_DIR%", output_dir).replace("%X86_GENERATION%", "$(location " + generation_config + ")" if generation_config else "").replace("%X86_SOURCES%", " ".join(["$(locations " + label + ")" for label in x86_sources])).replace("%X86_CONFIG%", "$(location configure_x86.py)" if x86 else "").replace("%ARCH%", architecture).replace("%ROLLBACK%", "" if x86 else "$(location //boot/bl33:rollback_provisioner_bin)").replace("$(location //boot/bl33:verifier_bin)", "$(location build_firmware.sh)" if x86 else "$(location //boot/bl33:verifier_bin)"),
    )
