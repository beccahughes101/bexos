#!/usr/bin/env python3
"""Bazel's explicit Trusty firmware snapshot boundary (a tar file, not a disk)."""

import hashlib
import io
import os
from pathlib import Path
import platform
import struct
import sys
import tarfile
import tempfile

ARTIFACTS = (
    "bl1.bin", "bl2.bin", "bl31.bin", "lk.bin", "lk.elf", "bl33.bin",
    "nt_fw_content.crt", "nt_fw_key.crt", "soc_fw_content.crt",
    "soc_fw_key.crt", "tb_fw.crt", "tos_fw_content.crt", "tos_fw_key.crt",
    "trusted_key.crt", "rpmb_dev", "RPMB_DATA",
    "bl33-rollback.bin", "rollback_nt_fw_content.crt",
)
X86_ARTIFACTS = ("lk.bin", "lk.elf", "boot.elf", "rpmb_dev", "RPMB_DATA")

def artifacts_for(architecture):
    if architecture == "aarch64": return ARTIFACTS
    if architecture == "x86_64": return X86_ARTIFACTS
    raise ValueError("unknown firmware architecture")

METADATA = "build.prototxt"
REFRESH = "bazel run //third_party/trusty:refresh_image"
MAX_ARTIFACT = 256 * 1024 * 1024
MAX_TRUSTY_IMAGE = 14 * 1024 * 1024  # QEMU secure DRAM from BL32_BASE to 0x0f000000


def digest(data):
    return hashlib.sha256(data).hexdigest()


def metadata(files, variant, architecture="aarch64"):
    artifacts = artifacts_for(architecture)
    version = 2 if architecture == "aarch64" else 1
    text = (f'format_version: {version}\nvariant: "{variant}"\n'
            f'host: "{platform.system()}-{platform.machine()}"\n')
    if architecture != "aarch64": text += f'architecture: "{architecture}"\n'
    for name in artifacts:
        data = files[name]
        text += (f'artifact {{ name: "{name}" size: {len(data)} '
                 f'sha256: "{digest(data)}" }}\n')
    return text.encode()


def read_bundle(path, variant="standard", architecture="aarch64"):
    artifacts = artifacts_for(architecture)
    files = {}
    with tarfile.open(path, "r:") as archive:
        for entry in archive:
            if (entry.name not in (*artifacts, METADATA) or entry.name in files
                    or not entry.isfile() or not 0 < entry.size <= MAX_ARTIFACT):
                raise ValueError(f"invalid bundle member {entry.name}")
            files[entry.name] = archive.extractfile(entry).read()
    if set(files) != set((*artifacts, METADATA)):
        raise ValueError("incomplete firmware bundle")
    if architecture == "aarch64" and len(files["lk.bin"]) >= MAX_TRUSTY_IMAGE:
        raise ValueError("Trusty image leaves no runtime space in QEMU secure RAM")
    # The pinned ARM kernel is position-independent (ET_DYN); x86 and the
    # Multiboot loader are fixed-address executables (ET_EXEC).
    expected = [("lk.elf", 2, 183 if architecture == "aarch64" else 62,
                 (2, 3) if architecture == "aarch64" else (2,))]
    if architecture == "x86_64":
        expected.append(("boot.elf", 1, 3, (2,)))
    for name, elf_class, machine, types in expected:
        data = files[name]
        if (len(data) < (64 if elf_class == 2 else 52) or data[:7] != b"\x7fELF" + bytes([elf_class, 1, 1])
                or struct.unpack_from("<H", data, 16)[0] not in types
                or struct.unpack_from("<H", data, 18)[0] != machine):
            raise ValueError(f"{name}: ELF architecture does not match {architecture} firmware")
    # This deliberately accepts only the canonical versioned manifest emitted
    # by this tool, including the host ABI of the bundled RPMB executable.
    if files[METADATA] != metadata(files, variant, architecture):
        raise ValueError("firmware hash, variant, format, or host ABI mismatch")
    return files


def write_bundle(destination, sources, variant="standard", architecture="aarch64"):
    artifacts = artifacts_for(architecture)
    files = {Path(source).name: Path(source).read_bytes() for source in sources}
    if len(sources) != len(artifacts) or set(files) != set(artifacts):
        raise ValueError("expected exactly one of each firmware artifact")
    destination = Path(destination)
    destination.parent.mkdir(parents=True, exist_ok=True)
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(dir=destination.parent, delete=False) as output:
            temporary = Path(output.name)
            with tarfile.open(fileobj=output, mode="w", format=tarfile.USTAR_FORMAT) as archive:
                for name, data in (*files.items(), (METADATA, metadata(files, variant, architecture))):
                    entry = tarfile.TarInfo(name)
                    entry.size = len(data)
                    entry.mode = 0o755 if name == "rpmb_dev" else 0o644
                    archive.addfile(entry, io.BytesIO(data))
            output.flush()
            os.fsync(output.fileno())
        read_bundle(temporary, variant, architecture)
        os.chmod(temporary, 0o644)
        os.replace(temporary, destination)
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def extract_bundle(destination, source, variant="standard", architecture="aarch64"):
    artifacts = artifacts_for(architecture)
    files = read_bundle(source, variant, architecture)
    destination = Path(destination)
    destination.mkdir(parents=True, exist_ok=True)
    for name, data in files.items():
        target = destination / name
        target.write_bytes(data)
        target.chmod(0o755 if name == "rpmb_dev" else 0o644)


def main(args):
    architecture = "aarch64"
    if args[:1] == ["--architecture"]:
        architecture = args[1]
        args = args[2:]
    artifacts = artifacts_for(architecture)
    variant = "standard"
    if args[:1] == ["--variant"]:
        variant = args[1]
        if variant not in ("standard", "authmgr_acceptance"):
            raise ValueError("unknown firmware variant")
        args = args[2:]
    operation, destination, *sources = args
    if operation == "pack":
        write_bundle(destination, sources, variant, architecture)
    elif operation == "extract":
        if len(sources) != 1:
            raise ValueError("saved Trusty image is missing")
        extract_bundle(destination, sources[0], variant, architecture)
    elif operation == "refresh":
        workspace = os.environ.get("BUILD_WORKSPACE_DIRECTORY")
        if not workspace:
            raise ValueError("refresh must be launched with bazel run")
        destination = Path(workspace) / destination
        # Validate before replacing a previous complete snapshot.
        files = read_bundle(sources[0], variant, architecture)
        with tempfile.TemporaryDirectory(dir=destination.parent) as staging:
            paths = []
            for name in artifacts:
                path = Path(staging) / name
                path.write_bytes(files[name])
                paths.append(path)
            write_bundle(destination, paths, variant, architecture)
        print(f"Saved Trusty firmware: {destination}")
    else:
        raise ValueError(f"unknown bundle operation {operation}")


if __name__ == "__main__":
    try:
        main(sys.argv[1:])
    except (OSError, ValueError, tarfile.TarError) as error:
        command = REFRESH if "authmgr_acceptance" not in sys.argv else "bazel run //third_party/trusty:refresh_authmgr_acceptance_image"
        if "x86_64" in sys.argv:
            command = "bazel run //third_party/trusty:refresh_x86_64_" + ("acceptance_image" if "authmgr_acceptance" in sys.argv else "image")
        sys.exit(f"Trusty image: {error}\nRefresh with: {command}")
