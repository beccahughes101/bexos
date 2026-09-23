/* BexOS's additional RAM exists only in the secure CPU address space.
 * It is deliberately outside the low legacy firmware/device memory map. */
#include "bexos_secure_layout.h"

static void bexos_secure_transition_ram(VirtMachineState *vms,
                                       MemoryRegion *secure_sysmem)
{
    const uint64_t volatile_bytes = BEXOS_PERSISTENT_BASE - BEXOS_TRANSITION_BASE;
    MemoryRegion *volatile_region = g_new(MemoryRegion, 1);
    MemoryRegion *persistent_region = g_new(MemoryRegion, 1);
    memory_region_init_ram(volatile_region, NULL, "bexos.secure-transition",
                           volatile_bytes, &error_fatal);
    memory_region_add_subregion(secure_sysmem, BEXOS_TRANSITION_BASE,
                                volatile_region);
    const char *path = g_getenv("BEXOS_SECURE_FIRMWARE");
    if (path && path[0]) {
        if (!memory_region_init_ram_from_file(
                persistent_region, NULL, "bexos.secure-firmware",
                BEXOS_PERSISTENT_BYTES, 0, RAM_SHARED | RAM_NAMED_FILE,
                path, 0, &error_fatal)) {
            abort();
        }
    } else {
        memory_region_init_ram(persistent_region, NULL,
                               "bexos.secure-firmware-volatile",
                               BEXOS_PERSISTENT_BYTES, &error_fatal);
    }
    memory_region_add_subregion(secure_sysmem, BEXOS_PERSISTENT_BASE,
                                persistent_region);
    const char *node = "/bexos-secure-transition";
    qemu_fdt_add_subnode(MACHINE(vms)->fdt, node);
    qemu_fdt_setprop_string(MACHINE(vms)->fdt, node, "compatible",
                            "bexos,secure-transition-v1");
    qemu_fdt_setprop_sized_cells(MACHINE(vms)->fdt, node, "reg", 2,
                                BEXOS_TRANSITION_BASE, 2, BEXOS_TRANSITION_BYTES);
    qemu_fdt_setprop_string(MACHINE(vms)->fdt, node, "status", "disabled");
    qemu_fdt_setprop_string(MACHINE(vms)->fdt, node, "secure-status", "okay");
}
