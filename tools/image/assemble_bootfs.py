#!/usr/bin/env python3
import argparse
import os
import struct
import sys
import subprocess

MAGIC = b"BEXBOOT\0"
VERSION = 1
HEADER = struct.Struct("<8sIIII")
ENTRY = struct.Struct("<256sQQ")
ALIGN = 4096


def align_up(value, alignment):
    return (value + alignment - 1) & ~(alignment - 1)


def parse_entry(spec):
    parts = spec.split("=", 1)
    if len(parts) != 2 or not parts[0].startswith("/"):
        raise ValueError(f"entry must be /boot/path=artifact, got {spec!r}")
    return parts[0], parts[1]


def build_data(entries):
    entries = sorted(entries)
    if len({path for path, _ in entries}) != len(entries):
        raise ValueError("duplicate BootFS path")
    table_size = ENTRY.size * len(entries)
    payload_offset = align_up(HEADER.size + table_size, ALIGN)
    table = bytearray()
    payload = bytearray()

    for path, data in entries:
        encoded = path.encode("utf-8")
        if len(encoded) > 255:
            raise ValueError(f"BootFS path is too long: {path}")
        offset = payload_offset + len(payload)
        table += ENTRY.pack(encoded + b"\0" * (256 - len(encoded)), offset, len(data))
        payload += data
        padding = align_up(len(payload), ALIGN) - len(payload)
        payload += b"\0" * padding

    return HEADER.pack(MAGIC, VERSION, len(entries), table_size, payload_offset) + table + (
        b"\0" * (payload_offset - HEADER.size - table_size)
    ) + payload


def build(entries):
    loaded = []
    for path, source in entries:
        with open(source, "rb") as handle:
            loaded.append((path, handle.read()))
    return build_data(loaded)


def parse_image(data):
    if len(data) < HEADER.size:
        raise ValueError("truncated BootFS header")
    magic, version, count, table_size, payload_offset = HEADER.unpack_from(data, 0)
    if magic != MAGIC or version != VERSION or table_size != count * ENTRY.size:
        raise ValueError("invalid BootFS header")
    if payload_offset > len(data) or HEADER.size + table_size > payload_offset:
        raise ValueError("invalid BootFS table")
    entries = []
    for index in range(count):
        raw, offset, length = ENTRY.unpack_from(data, HEADER.size + index * ENTRY.size)
        end = offset + length
        if offset < payload_offset or end > len(data):
            raise ValueError("invalid BootFS entry bounds")
        path = raw.split(b"\0", 1)[0].decode("utf-8")
        entries.append((path, data[offset:end]))
    return entries


def validate_elf(header, architecture):
    if (len(header) < 64 or header[:7] != b"\x7fELF\x02\x01\x01"
            or struct.unpack_from("<HH", header, 16) != (2, {"aarch64": 183, "x86_64": 62}[architecture])):
        raise ValueError(f"expected ELF64 little-endian {architecture} executable")


def self_test():
    import tempfile

    for architecture, machine in [("aarch64", 183), ("x86_64", 62)]:
        header = bytearray(64)
        header[:7] = b"\x7fELF\x02\x01\x01"
        struct.pack_into("<HH", header, 16, 2, machine)
        validate_elf(header, architecture)
        try:
            validate_elf(header, "aarch64" if architecture == "x86_64" else "x86_64")
        except ValueError:
            pass
        else:
            raise AssertionError("wrong guest architecture accepted")
        for malformed in [header[:32], b"not ELF" + header[7:]]:
            try:
                validate_elf(malformed, architecture)
            except ValueError:
                pass
            else:
                raise AssertionError("malformed ELF accepted")
    with tempfile.TemporaryDirectory() as root:
        one = os.path.join(root, "one.bin")
        two = os.path.join(root, "two.bin")
        with open(one, "wb") as handle:
            handle.write(b"one")
        with open(two, "wb") as handle:
            handle.write(b"two-two")
        image = build([("/boot/one", one), ("/boot/two", two)])
        magic, version, count, table_size, payload_offset = HEADER.unpack_from(image, 0)
        assert magic == MAGIC
        assert version == VERSION
        assert count == 2
        assert table_size == ENTRY.size * 2
        assert payload_offset % ALIGN == 0
        first = ENTRY.unpack_from(image, HEADER.size)
        second = ENTRY.unpack_from(image, HEADER.size + ENTRY.size)
        assert image[first[1] : first[1] + first[2]] == b"one"
        assert image[second[1] : second[1] + second[2]] == b"two-two"
        parsed = parse_image(image)
        assert parsed == [("/boot/one", b"one"), ("/boot/two", b"two-two")]
        try:
            build_data(parsed + [("/boot/one", b"collision")])
        except ValueError as error:
            assert "duplicate" in str(error)
        else:
            raise AssertionError("duplicate overlay destination accepted")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--out")
    parser.add_argument("--base")
    parser.add_argument("--package-tree", action="append", default=[])
    parser.add_argument("--manifest-validator")
    parser.add_argument("--architecture", choices=["aarch64", "x86_64"], default="aarch64")
    parser.add_argument("--entry", action="append", default=[])
    parser.add_argument("--elf", action="append", default=[])
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()

    if args.self_test:
        self_test()
        return 0
    if not args.out:
        parser.error("--out is required")

    entries = [parse_entry(spec) for spec in args.entry]
    for spec in args.elf:
        path, source = parse_entry(spec)
        with open(source, "rb") as handle:
            header = handle.read(64)
        try:
            validate_elf(header, args.architecture)
        except ValueError as error:
            parser.error(f"{source}: {error}")
        entries.append((path, source))
    manifests = [(path, source) for path, source in entries if path.endswith('/package.bexmanifest')]
    if manifests and not args.manifest_validator:
        parser.error('package manifests require --manifest-validator')
    for path, source in manifests:
        prefix = path.rsplit('/', 1)[0] + '/'
        payloads = [payload for name, payload in entries if name.startswith(prefix) and name != path]
        subprocess.run([args.manifest_validator, 'validate', args.architecture.upper(), source] + payloads, check=True)
    if args.base:
        if entries:
            parser.error("--base cannot be combined with --entry or --elf")
        with open(args.base, "rb") as handle:
            merged = parse_image(handle.read())
        for spec in args.package_tree:
            package, root = spec.split("=", 1) if "=" in spec else ("", "")
            if not package or not root or any(c not in "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._-:" for c in package):
                parser.error(f"invalid --package-tree {spec!r}")
            for directory, names, files in os.walk(root):
                names.sort()
                files.sort()
                for name in files:
                    source = os.path.join(directory, name)
                    relative = os.path.relpath(source, root).replace(os.sep, "/")
                    with open(source, "rb") as handle:
                        merged.append((f"/boot/pkg/{package}/{relative}", handle.read()))
        with open(args.out, "wb") as handle:
            handle.write(build_data(merged))
    else:
        if args.package_tree:
            parser.error("--package-tree requires --base")
        with open(args.out, "wb") as handle:
            handle.write(build(entries))
    return 0


if __name__ == "__main__":
    sys.exit(main())
