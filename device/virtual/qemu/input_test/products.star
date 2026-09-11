load("//device/virtual/qemu/base:configs.star", "qemu_system")

def virtual_aarch64():
    return qemu_system("input_test", "aarch64", graphics = True)

def virtual_x86_64():
    return qemu_system("input_test", "x86_64", graphics = True)

def virtual_x86_64_development():
    return qemu_system("input_test", "x86_64", graphics = True, development = True)
