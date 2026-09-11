#!/usr/bin/env python3
"""Validate an x86 ELF and embed its load segments into a Multiboot loader."""
from pathlib import Path
import struct
import sys


def inspect(data, trusty):
    if len(data) < 64 or data[:7] != b'\x7fELF\x02\x01\x01':
        raise ValueError('payload must be a little-endian ELF64')
    kind, machine = struct.unpack_from('<HH', data, 16)
    if kind != 2 or machine != 62:
        raise ValueError('payload must be an executable x86_64 ELF')
    entry, phoff = struct.unpack_from('<QQ', data, 24)
    phsize, count = struct.unpack_from('<HH', data, 54)
    if phsize != 56 or not 0 < count <= 64 or phoff+count*phsize > len(data):
        raise ValueError('invalid ELF program headers')
    segments = []
    physical_entry = None
    for i in range(count):
        typ, flags, offset, virt, phys, filesz, memsz, align = struct.unpack_from('<IIQQQQQQ', data, phoff+i*phsize)
        if typ != 1 or memsz == 0:
            continue
        if filesz > memsz or offset+filesz > len(data) or phys < 0x200000 or phys+memsz > 0x30000000:
            raise ValueError('ELF segment outside payload RAM or file')
        if any(phys < a+m and a < phys+memsz for a, _, _, m in segments):
            raise ValueError('overlapping ELF segments')
        segments.append((phys, offset, filesz, memsz))
        if flags & 1 and virt <= entry < virt+filesz:
            physical_entry = phys+entry-virt
        elif trusty and flags & 1 and phys <= entry < phys+filesz:
            # The pinned x86 Trusty linker uses ENTRY(_start - KERNEL_BASE +
            # MEMBASE), while PT_LOAD virtual addresses remain in the high half.
            physical_entry = entry
    if physical_entry is None:
        raise ValueError('entry point is not in executable file-backed memory')
    if not trusty:
        # Multiboot2 requires a header in the first 32KiB, aligned to eight bytes.
        valid = False
        for offset in range(0, min(len(data)-16, 32768), 8):
            magic, arch, length, checksum = struct.unpack_from('<IIII', data, offset)
            if magic == 0xe85250d6 and arch == 0 and length >= 24 and offset+length <= min(len(data), 32768) and (magic+arch+length+checksum)&0xffffffff == 0:
                cursor = offset + 16
                end = offset + length
                while cursor + 8 <= end:
                    kind, flags, size = struct.unpack_from('<HHI', data, cursor)
                    if size < 8 or cursor + size > end or flags & ~1:
                        raise ValueError('invalid Multiboot2 header tag')
                    if kind == 0:
                        valid = size == 8 and cursor + size == end
                        break
                    if kind == 1:
                        if size % 4:
                            raise ValueError('invalid Multiboot2 information request')
                        requests = struct.unpack_from('<' + 'I' * ((size - 8) // 4), data, cursor + 8)
                        if not flags & 1 and any(tag not in (3, 4, 6) for tag in requests):
                            raise ValueError('unsupported required Multiboot2 information')
                    elif not flags & 1:
                        raise ValueError('unsupported required Multiboot2 header tag')
                    cursor += (size + 7) & ~7
                break
        if not valid:
            raise ValueError('BexOS payload has no valid Multiboot2 header')
    return physical_entry, segments


def pack(source, directory, mode):
    data = Path(source).read_bytes()
    entry, segments = inspect(data, mode == 'trusty')
    directory = Path(directory)
    directory.mkdir(parents=True, exist_ok=True)
    (directory/'payload.bin').write_bytes(data)
    (directory/'payload.h').write_text(
        f'#define PAYLOAD_ENTRY {entry}u\n#define PAYLOAD_TRUSTY {int(mode == "trusty")}\n'
        f'#define PAYLOAD_SEGMENTS {len(segments)}\n'
        'struct segment { unsigned address, offset, filesz, memsz; };\n'
        'static const struct segment segments[] = {\n' +
        ''.join('{%du,%du,%du,%du},\n' % s for s in segments) + '};\n')
    (directory/'payload.S').write_text('.section .rodata.payload,"a"\n.global payload_start\npayload_start:\n.incbin "'+str((directory/'payload.bin').resolve())+'"\n.section .note.GNU-stack,"",@progbits\n')


if __name__ == '__main__':
    pack(*sys.argv[1:])
