load("//device/virtual/qemu/base:configs.star", "qemu_system")

def virtual_aarch64():
    return qemu_system("nongui", "aarch64", graphics = False)

def virtual_x86_64():
    return qemu_system("nongui", "x86_64", graphics = False)

def virtual_x86_64_development():
    return qemu_system("nongui", "x86_64", graphics = False, development = True)
