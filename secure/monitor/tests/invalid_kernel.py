"""Bazel-only construction of signed, invalid kernel load-layout fixtures."""
from pathlib import Path
import struct
import sys


def main():
    source, destination, variant = sys.argv[1:]
    image = bytearray(Path(source).read_bytes())
    assert image[:7] == b'\x7fELF\x02\x01\x01'
    if variant == 'architecture':
        struct.pack_into('<H', image, 18, 183)
    else:
        target = {'handoff': 0x01000000, 'bootfs': 0x08000000,
                  'evidence': 0x07f00000, 'update': 0x10000000}[variant]
        offset = struct.unpack_from('<Q', image, 32)[0]
        size, count = struct.unpack_from('<HH', image, 54)
        assert size == 56 and 0 < count <= 64
        headers = [offset + index * size for index in range(count)
                   if struct.unpack_from('<I', image, offset + index * size)[0] == 1]
        start = min(struct.unpack_from('<Q', image, header + 24)[0] for header in headers)
        delta = target - start
        entry = struct.unpack_from('<Q', image, 24)[0]
        for header in headers:
            address = struct.unpack_from('<Q', image, header + 24)[0]
            length = struct.unpack_from('<Q', image, header + 40)[0]
            if address <= entry < address + length:
                struct.pack_into('<Q', image, 24, entry + delta)
            struct.pack_into('<Q', image, header + 24, address + delta)
    Path(destination).write_bytes(image)


if __name__ == '__main__':
    main()
