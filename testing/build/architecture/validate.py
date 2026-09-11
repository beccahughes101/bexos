"""Validate actual cross-linked guest code and its selected product sources."""

import os
from pathlib import Path
import struct

arch = os.environ["BEXOS_QEMU_ARCH"]
machine = {"aarch64": 183, "x86_64": 62}[arch]
data = Path(os.environ["ELF_PROBE"]).read_bytes()
assert len(data) >= 64 and data[:7] == b"\x7fELF\x02\x01\x01", "expected ELF64"
kind, actual_machine = struct.unpack_from("<HH", data, 16)
assert actual_machine == machine, "wrong guest ELF machine"
assert kind == 2, "probe must cross-link as an executable"

device = Path(os.environ["DEVICE_CONFIG"]).read_text()
policy = Path(os.environ["PLATFORM_CONFIG"]).read_text()
hardware = Path(os.environ["HARDWARE_CONFIG"]).read_text()
architecture_field = "architecture: ARCH_" + arch.upper()
assert architecture_field in device, "wrong device architecture"
assert architecture_field in policy, "wrong policy architecture"

if arch == "x86_64":
    assert all(value not in hardware for value in ("/arm/", "pl011", "pl031")), (
        "ARM hardware in x86 product"
    )
    assert "has_acpi: true" in device
    assert "secure_world: TRUSTY" in device, "integrated x86 must select real Trusty"
    assert "secure_world: EMULATED" not in device, "software TEE in secure product"
    assert "enforce_secure_boot: true" in policy
    assert "rpmb_anti_rollback: true" in policy
    assert "secure_storage_backend: QEMU_EMULATED_RPMB" in policy
else:
    assert "pl011" in hardware and "pl031" in hardware, "ARM hardware regression"

print(f"{arch}: C/C++/Rust ELF and selected product configuration verified")
