load("//device/virtual/qemu/base:configs.star", "qemu_system")

def virtual_aarch64():
    return qemu_system("sdk_acceptance", "aarch64", graphics = False, workstation_services = False)

def virtual_x86_64():
    return qemu_system("sdk_acceptance", "x86_64", graphics = False, workstation_services = False)

def virtual_x86_64_development():
    return qemu_system("sdk_acceptance", "x86_64", graphics = False, development = True, workstation_services = False)
