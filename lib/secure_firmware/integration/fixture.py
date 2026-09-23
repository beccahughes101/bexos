"""Small signed storage-test payload; it is never used as executable firmware."""
from pathlib import Path
import struct
import sys

arm = len(sys.argv) == 3 and sys.argv[2] == "aarch64"
machine = 183 if arm else 62
image_type = 3 if arm else 2
image = bytearray(128)
image[:7] = b"\x7fELF\x02\x01\x01"
struct.pack_into("<HHIQQQIHHHHHH", image, 16, image_type, machine, 1, 0x200078, 64, 0, 0, 64, 56, 1, 64, 0, 0)
struct.pack_into("<IIQQQQQQ", image, 64, 1, 5, 0, 0x200000, 0x200000, 128, 128, 4096)
image[120:] = bytes([0x90] * 8)
Path(sys.argv[1]).write_bytes(image)
