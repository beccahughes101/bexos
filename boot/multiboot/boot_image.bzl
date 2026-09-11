"""Bazel-owned Multiboot boot image for QEMU and native Multiboot2 loaders."""
def multiboot_image(name, kernel, trusty = False, output = None, **kwargs):
    native.genrule(
        name = name,
        srcs = [kernel, "//boot/multiboot:pack.py", "//boot/multiboot:loader.c", "//boot/multiboot:entry.S", "//boot/multiboot:loader.ld"],
        outs = [output or name + ".elf"],
        tools = ["@llvm//tools:clang", "@llvm//tools:ld.lld"],
        cmd = """
set -eu
stage="$(@D)/%s.stage"
python3 $(location //boot/multiboot:pack.py) $(location %s) "$$stage" %s
$(location @llvm//tools:clang) --target=i386-unknown-none-elf -ffreestanding -fno-pic -fno-pie -fno-stack-protector -fno-builtin -mno-sse -mno-mmx -Os -I"$$stage" -c $(location //boot/multiboot:loader.c) -o "$$stage/loader.o"
$(location @llvm//tools:clang) --target=i386-unknown-none-elf -c $(location //boot/multiboot:entry.S) -o "$$stage/entry.o"
$(location @llvm//tools:clang) --target=i386-unknown-none-elf -c "$$stage/payload.S" -o "$$stage/payload.o"
$(location @llvm//tools:ld.lld) -m elf_i386 -T $(location //boot/multiboot:loader.ld) "$$stage/loader.o" "$$stage/entry.o" "$$stage/payload.o" -o "$@"

""" % (name, kernel, "trusty" if trusty else "bexos"),
        **kwargs
    )
