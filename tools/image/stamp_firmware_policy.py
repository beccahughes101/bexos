"""Bind an x86 policy to the exact validated saved secure-runtime image."""
from pathlib import Path
import hashlib
import re
import struct
import sys


def stamp(source, image):
    if not 64 <= len(image) <= 128 * 1024 * 1024:
        raise ValueError('secure firmware size is outside the supported bound')
    if image[:7] != b'\x7fELF\x02\x01\x01' or struct.unpack_from('<HH', image, 16) != (2, 62):
        raise ValueError('secure firmware must be an executable x86_64 ELF')
    digest = hashlib.sha256(image).hexdigest()
    result, count = re.subn(
        r'(?m)^(\s*orchestrator_verification:\s*)"sha256:[0-9a-f]{64}"\s*$',
        lambda match: match[1] + '"sha256:' + digest + '"', source)
    if count != 1:
        raise ValueError('policy must contain exactly one firmware measurement')
    return result


if __name__ == '__main__':
    source, image, output = map(Path, sys.argv[1:])
    output.write_text(stamp(source.read_text(), image.read_bytes()))
