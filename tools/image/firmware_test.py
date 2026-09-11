"""Exercise the same authenticated parser used by the secure execution owner."""
import os
from pathlib import Path
import struct
import subprocess
import sys

tool, key = sys.argv[1:]
directory = Path(os.environ["TEST_TMPDIR"]) / "firmware"
directory.mkdir()


def command(*args, accepted=True):
    result = subprocess.run([tool, *map(str, args)], capture_output=True)
    if (result.returncode == 0) != accepted:
        raise AssertionError((args, result.returncode, result.stderr.decode()))


for arch, machine, component in [
    ("x86_64", 62, "trusty"),
    ("x86_64", 62, "hypervisor"),
    ("aarch64", 183, "trusty"),
]:
    base = directory / (arch + component)
    image = bytearray(128 * 1024)  # Exceeds a single shared-memory registration.
    image[:7] = b"\x7fELF\x02\x01\x01"
    struct.pack_into("<HH", image, 16, 2, machine)
    image_path = base.with_suffix(".elf")
    image_path.write_bytes(image)
    bundle = base.with_suffix(".fw")
    root = base.with_suffix(".key")
    command("--firmware", key, bundle, root, 4, arch, component, image_path)
    command("--verify-firmware", bundle, root, arch, component, 3, 4)
    command("--verify-firmware", bundle, root, arch, component, 4, 4, accepted=False)
    command("--verify-firmware", bundle, root, arch, component, 3, 5, accepted=False)
    other_arch = "aarch64" if arch == "x86_64" else "x86_64"
    command("--verify-firmware", bundle, root, other_arch, component, 3, 4, accepted=False)
    other_component = "hypervisor" if component == "trusty" else "trusty"
    command("--verify-firmware", bundle, root, arch, other_component, 3, 4, accepted=False)
    original = bundle.read_bytes()
    for name, changed in [
        ("appended", original + b"\x00"),
        ("truncated", original[:-1]),
        ("payload", original[:-1] + b"\x01"),
        ("signature", original[:320] + bytes([original[320] ^ 1]) + original[321:]),
        ("reserved", original[:24] + b"\x01" + original[25:]),
        ("overflow", original[:8] + b"\xff" * 8 + original[16:]),
    ]:
        invalid = base.with_suffix("." + name)
        invalid.write_bytes(changed)
        command("--verify-firmware", invalid, root, arch, component, 3, 4, accepted=False)
    wrong_root = base.with_suffix(".wrong-key")
    changed = bytearray(root.read_bytes())
    changed[50] ^= 1
    wrong_root.write_bytes(changed)
    command("--verify-firmware", bundle, wrong_root, arch, component, 3, 4, accepted=False)
print("secure firmware: architecture, component, generation, signature and exact-image checks passed")
