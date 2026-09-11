"""Authenticate the diagnostic monitor as part of the EFI loader image."""

def authenticated_monitor(name, monitor, domain_banks = []):
    native.genrule(
        name = name + "_unsigned",
        srcs = [monitor, "embed_monitor.py", "monitor_loader.c", "probe.c", "efi.h", "//boot/multiboot:pack.py"],
        outs = [name + ".unsigned.efi"],
        tools = ["@llvm//tools:clang", "@llvm//tools:lld-link"],
        cmd = """
set -eu
stage="$(@D)/%s.stage"
python3 -B $(location embed_monitor.py) $(location %s) "$$stage" $(location //boot/multiboot:pack.py) %s
$(location @llvm//tools:clang) --target=x86_64-pc-windows-msvc -ffreestanding -fno-stack-protector -fno-builtin -mno-red-zone -Os -DMONITOR_PAYLOAD -c $(location probe.c) -o "$$stage/probe.obj"
$(location @llvm//tools:clang) --target=x86_64-pc-windows-msvc -ffreestanding -fno-stack-protector -fno-builtin -mno-red-zone -Os -I"$$stage" -c $(location monitor_loader.c) -o "$$stage/loader.obj"
$(location @llvm//tools:clang) --target=x86_64-pc-windows-msvc -c "$$stage/monitor_payload.S" -o "$$stage/payload.obj"
$(location @llvm//tools:lld-link) /subsystem:efi_application /entry:efi_main /nodefaultlib /machine:x64 /out:$@ "$$stage/probe.obj" "$$stage/loader.obj" "$$stage/payload.obj"
""" % (name, monitor, " ".join([str(value) for bank in domain_banks for value in bank])),
    )
    native.genrule(
        name = name,
        srcs = [":" + name + "_unsigned", ":development.pem", "//device/virtual/qemu/base:keys/avb_dev_rsa_private.pem"],
        outs = [name + ".signed.efi"],
        tools = ["//third_party/osslsigncode"],
        cmd = "$(location //third_party/osslsigncode) sign -h sha256 -certs $(location :development.pem) " +
              "-key $(location //device/virtual/qemu/base:keys/avb_dev_rsa_private.pem) -in $(location :%s_unsigned) -out $@" % name,
    )
