"""Check the assembled guest closure, including ELF shared libraries."""
from pathlib import Path
import os
import struct

image = Path(os.environ["BOOTFS_IMAGE"]).read_bytes()
architecture = os.environ["BEXOS_QEMU_ARCH"]
machine = {"aarch64": 183, "x86_64": 62}[architecture]
magic, version, count, table_size, payload = struct.unpack_from("<8sIIII", image)
assert magic == b"BEXBOOT\0" and version == 1
assert table_size == count * 272 and 24 + table_size <= payload <= len(image)
paths = set()
executables = set()
product = os.environ.get("BEXOS_QEMU_PRODUCT", "nongui")
assembly = None
for index in range(count):
    name, offset, size = struct.unpack_from("<256sQQ", image, 24 + index * 272)
    name = name.split(b"\0", 1)[0].decode()
    assert name not in paths and offset >= payload and offset + size <= len(image)
    paths.add(name)
    artifact = memoryview(image)[offset:offset + size]
    if name == "/boot/manifest/product.assembly":
        assembly = bytes(artifact).decode()
    if bytes(artifact[:4]) == b"\x7fELF":
        assert len(artifact) >= 64 and bytes(artifact[:7]) == b"\x7fELF\x02\x01\x01", name
        kind, actual_machine = struct.unpack_from("<HH", artifact, 16)
        assert kind in (2, 3) and actual_machine == machine, f"wrong ELF architecture: {name}"
        executables.add(name)
    elif name.endswith(".wasm"):
        # Guest applications include component-model WASM as well as core modules.
        assert bytes(artifact[:8]) in (
            b"\x00asm\x01\x00\x00\x00",
            b"\x00asm\x0d\x00\x01\x00",
        ), f"invalid WASM module or component: {name}"
    elif "/bin/" in name or name.endswith(".so"):
        raise AssertionError(f"native artifact is not ELF: {name}")

assert "/boot/pkg/bexos.platform.appd/bin/appd" in executables
assert "/boot/pkg/bexos.service.teed/bin/teed" in executables
tee = os.environ.get("BEXOS_QEMU_TEE", "trusty")
assert f"/boot/pkg/bexos.lib.tee_driver.{tee}/lib/libbexos_tee_driver.so" in executables
assert "/boot/pkg/bexos.driver.serial.virtio_console/bin/virtio_console_driver" in executables
assert assembly is not None, "missing product assembly index"
assert f"product_name: {product}_{architecture}\n" in assembly
assert f"board: //device/virtual/qemu/base/{architecture}\n" in assembly
assert f"platform_config: //device/virtual/qemu/{product}:platform_config_bin\n" in assembly
assert "/boot/platform.pcfg" in paths
arm_devices = [path for path in paths if "pl011" in path or "pl031" in path]
assert bool(arm_devices) == (architecture == "aarch64"), "wrong hardware in product closure"
print(f"{architecture}: checked {len(paths)} BootFS entries and {len(executables)} ELF artifacts")
