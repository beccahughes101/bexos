#!/usr/bin/env python3
"""Validate AArch64 native ELF hardening properties."""

import os
import struct
import sys

PT_LOAD = 1
PT_NOTE = 4
PF_X = 1
PF_W = 2
EM_AARCH64 = 183
NT_GNU_PROPERTY_TYPE_0 = 5
GNU_PROPERTY_AARCH64_FEATURE_1_AND = 0xC0000000
GNU_PROPERTY_AARCH64_FEATURE_1_BTI = 1 << 0
GNU_PROPERTY_AARCH64_FEATURE_1_PAC = 1 << 1
REQUIRED_AARCH64_FEATURES = (
    GNU_PROPERTY_AARCH64_FEATURE_1_BTI | GNU_PROPERTY_AARCH64_FEATURE_1_PAC
)


class ElfError(Exception):
    pass


def align(value, boundary):
    return (value + boundary - 1) & ~(boundary - 1)


def collect_paths(argv):
    paths = []
    for arg in argv:
        if os.path.isdir(arg):
            for root, _, files in os.walk(arg):
                for name in files:
                    paths.append(os.path.join(root, name))
        else:
            paths.append(arg)
    return paths


def read_elf(path):
    with open(path, "rb") as f:
        data = f.read()
    if len(data) < 64 or data[:4] != b"\x7fELF":
        return None
    if data[4] != 2 or data[5] != 1:
        raise ElfError("only ELF64 little-endian artifacts are supported")
    if struct.unpack_from("<H", data, 18)[0] != EM_AARCH64:
        return None
    return data


def program_headers(data):
    phoff = struct.unpack_from("<Q", data, 32)[0]
    phentsize = struct.unpack_from("<H", data, 54)[0]
    phnum = struct.unpack_from("<H", data, 56)[0]
    if phentsize < 56:
        raise ElfError("invalid program-header entry size")
    for index in range(phnum):
        off = phoff + index * phentsize
        if off + 56 > len(data):
            raise ElfError("program header outside file")
        fields = struct.unpack_from("<IIQQQQQQ", data, off)
        yield {
            "type": fields[0],
            "flags": fields[1],
            "offset": fields[2],
            "filesz": fields[5],
        }


def note_properties(data, ph):
    start = ph["offset"]
    end = start + ph["filesz"]
    if end > len(data):
        raise ElfError("note segment outside file")
    cursor = start
    while cursor + 16 <= end:
        namesz, descsz, ntype = struct.unpack_from("<III", data, cursor)
        cursor += 12
        name_start = cursor
        name_end = cursor + namesz
        cursor = align(name_end, 4)
        desc_start = cursor
        desc_end = cursor + descsz
        cursor = align(desc_end, 4)
        if desc_end > end:
            raise ElfError("note descriptor outside segment")
        if ntype != NT_GNU_PROPERTY_TYPE_0:
            continue
        if data[name_start:name_end] != b"GNU\x00":
            continue
        prop = desc_start
        while prop + 8 <= desc_end:
            prop_type, prop_size = struct.unpack_from("<II", data, prop)
            value_start = prop + 8
            value_end = value_start + prop_size
            if value_end > desc_end:
                raise ElfError("GNU property outside descriptor")
            if prop_type == GNU_PROPERTY_AARCH64_FEATURE_1_AND and prop_size >= 4:
                yield struct.unpack_from("<I", data, value_start)[0]
            prop = align(value_end, 8)


def validate(path):
    data = read_elf(path)
    if data is None:
        return "skipped"
    features = 0
    for ph in program_headers(data):
        if ph["type"] == PT_LOAD and ph["flags"] & PF_W and ph["flags"] & PF_X:
            raise ElfError("contains a W+X PT_LOAD segment")
        if ph["type"] == PT_NOTE:
            for value in note_properties(data, ph):
                features |= value
    if features & REQUIRED_AARCH64_FEATURES != REQUIRED_AARCH64_FEATURES:
        raise ElfError("missing GNU AArch64 BTI/PAC property")
    return "validated"


def main(argv):
    paths = collect_paths(argv)
    if not paths:
        print("usage: validate <elf-or-directory> [...]", file=sys.stderr)
        return 2
    validated = 0
    errors = []
    for path in paths:
        try:
            if validate(path) == "validated":
                validated += 1
        except (OSError, ElfError) as exc:
            errors.append(f"{path}: {exc}")
    if errors:
        for error in errors:
            print(error, file=sys.stderr)
        return 1
    print(f"validated {validated} AArch64 ELF artifact(s)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
